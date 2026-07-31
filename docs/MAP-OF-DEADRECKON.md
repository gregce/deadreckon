# Map of DeadReckon

Status: repository analysis updated 2026-07-31 through Watchkeeper durable
continuation, protected operator capture, trust-boundary hardening and
credential-free adversarial trials.

## Executive map

DeadReckon is not primarily another coding agent. It is an outer control plane for coding agents and agent harnesses. The inner harness proposes and performs work; DeadReckon owns the durable run, isolation, budgets, evidence, independent completion decision, recovery, and promotion.

Its irreducible pattern is:

```text
goal + definition of done
          |
          v
isolated, durable Job -----> provider / inner harness does work
          |                              |
          +<---- events, spend, diffs, snapshots, provenance
          |
          v
contained checks + independent semantic judge
          |
          +---- fail/revise: retain evidence, continue or stop explicitly
          |
          +---- pass: seal receipt, then promote deliberately
```

That is the essential product. The strongest reason for it to exist is the gap between *an agent saying it is done* and *an operator being able to trust, inspect, recover, and accept the result*. This matches the original unmet-needs research: predictable delegation needed operational infrastructure more than another transcript or chat surface.

The repository now contains three different things which should not be judged alike:

1. A differentiated Job kernel: immutable authority, append-only lifecycle,
   fenced local supervision and isolated work across guided and ordinary
   direct execution. Durable Single, Graph and Campaign Jobs prove 2
   independent completion decisions before deliberate promotion. Preserve and
   dogfood this.
2. Established graph/campaign conductors, direct advanced commands, reports,
   imports, and narration. Ordinary direct orchestration, new chains,
   stored-plan forks and campaigns now run those conductors under a parent Job
   and verify the merged parent. Explicit compatibility modes and chain
   extension remain process-owned. Public and guided run follow-ups use a
   parent-bound durable Single Job.
3. Compatibility layers, speculative depth, and incomplete bets. Deprecate or
   quarantine these rather than allowing them to obscure the kernel.

## How maturity is assessed

Maturity here means engineering evidence in this repository, not market adoption.

| Label | Meaning |
|---|---|
| **Hardened** | Central path, persistent model, broad tests, recovery/error semantics and established user surface. |
| **Stable / bounded** | Real and tested, but deliberately narrower than the whole unmet need. |
| **Maturing** | Substantive implementation with a known parity, safety, migration or operability gap. |
| **Experimental** | Working implementation whose product demand or abstraction boundary is not yet established. |
| **Compatibility-only** | Retained for old scripts or naming; not a product direction. |
| **Absent** | The underlying need is not currently addressed, even if nearby primitives exist. |

The repository has extensive automated coverage, but it has no product-usage telemetry. A feature can therefore be engineering-mature and still be unvalidated as something operators need.

## Architecture: the six layers

| Layer | Main location | Responsibility | Product significance |
|---|---|---|---|
| Operator surface | `crates/deadreckon` | CLI, guided `start`, Course, lifecycle commands, attach, orchestration entry points | Keeps the default flow approachable while exposing advanced control. |
| Runtime | `crates/deadreckon-runtime` | Outer turn loop, provider/sandbox composition, process execution, flight recording, docs and seams | Enforces policy around an inner harness rather than trusting that harness to supervise itself. |
| Core | `crates/deadreckon-core` | Job event store/reducer, fenced leases, process groups, receipts, paths, run state, gates, promotion and `RunView`/`JobView` | Durable source of truth and most of the product's invariants. |
| Providers | `crates/deadreckon-providers` | Provider trait/router, direct HTTP models, concrete CLIs, descriptor-driven CLIs and Codex app-server support | Makes the control vocabulary portable across changing agent harnesses. |
| Sandbox | `crates/deadreckon-sandbox` | Seatbelt, bubblewrap, Docker and explicit no-sandbox execution | Creates the capability boundary between delegated work and the host. |
| Protocol | `crates/deadreckon-protocol` | Versioned run ledgers plus checked Job, lease, authority, semantic-judgment and completion-receipt wire types | Separates durable evidence from implementation internals. |

The intended dependency direction is operator surface -> runtime -> provider/sandbox/core -> protocol. Keel moved the persisted ledger types, readers and writers onto that downward dependency and added checked schemas without changing the stored bytes.

### Two execution engines, one outer contract

The provider abstraction deliberately spans two materially different inner loops:

- Direct APIs return one structured action such as `bash`, `write_file`, `reshape` or `done`; the DeadReckon runtime executes it.
- CLI harnesses such as Codex or Claude mutate the working tree themselves; DeadReckon observes them, records evidence, snapshots changes and judges completion afterward.

Provider descriptors turn compatible CLIs into configuration rather than one bespoke Rust implementation per tool. This is a genuinely important abstraction: the stable product is the outer contract, not any vendor's current command-line syntax.

### One verified Job is the compositional atom

Guided `start` and ordinary direct execution now prove the stronger atom: one
approved Job, one append-only lifecycle, one same-ID parent result, contained
deterministic checks, one fresh semantic judgment, and one signed receipt.

Direct `run` creates a durable Single Job. Direct `orchestrate`, stored-plan
`fork`, and new supported chains create durable Graph Jobs. Direct `campaign`
creates a durable Campaign Job. A Graph always merges at the end. A Campaign
can recover exact persisted sub-plans and revalidates its worst-of roll-up. The
supervisor then verifies, receipts and promotes the same-ID parent result.
Preview, explicit in-place/uncontained execution, historical `chain
run|resume`, and chain extension remain process-owned and cannot issue a
trusted Job receipt. Public `extend` and a follow-up selected by guided
`start` freeze the completed parent state, promoted-artifact tree and verified
receipt, then queue a durable Single Job.

Graph and Campaign parents can now act on semantic `revise` through bounded,
fenced parent-only repair. The next consolidation step is dogfooding that
promise and retiring compatibility modes whose semantics can be represented
truthfully by Jobs.

## Capability portfolio

| Capability | Need and abstracted pattern | How it works | What is distinctive | Maturity | Disposition |
|---|---|---|---|---|---|
| Guided `start` and Course | Turn an underspecified request into a bounded run contract | Interactive/non-interactive intake selects codebase, provider, budget and done conditions; Course can decompose scope before launch | A friendly entry to a rigorous outer harness | **Maturing**: strong surface, but accepted Course pieces influence count while the plan can re-decompose them | **Preserve and align**; keep planning advisory and make accepted decomposition authoritative enough to avoid surprise |
| Definition-of-done compiler | Make acceptance executable instead of conversational | `def-done` and setup infer or generate the independent gate command and persist the contract | Converts repository conventions into a machine-checkable boundary | **Stable / bounded** | **Essential; extend** with rules-as-gate and clearer explanations |
| Persisted run kernel | Retain work and evidence when a process fails | A run directory stores state, working tree, snapshots, events and evidence | File-backed facts survive their original terminal | **Hardened as persistence**, not by itself a relaunch guarantee | **Essential; keep rich run evidence below Job control** |
| Durable Job supervisor | Approve before work, detach safely and stop for typed reasons | Guided and ordinary direct single, review, full-plan, chain, stored-plan fork and campaign launches write immutable authority and one append-only parent Job, then a fenced lease supervises the root process group | Separates process exit from lifecycle truth and gives one durable parent ID | **Stable / bounded**: supported new launches can earn a verified receipt; explicit compatibility modes remain and the live reboot drill is outstanding | **Highest-priority dogfood; close live recovery proof** |
| Lifecycle control | Start, observe, cancel and finish Jobs coherently | The shared resolver includes Jobs; `list` suppresses backing duplicates; status/attach/kill/finish use typed outcome and stop reason | Ordinary operations read persisted control truth | **Stable / bounded**: the cross-kind journey and Job resolver are tested | **Preserve one resolver and one Job event truth** |
| Codebase isolation | Contain changes and make promotion deliberate | Worktree, copy, fresh-repo and in-place modes separate the agent's workspace from the user's target | Isolation is part of the run contract, not a provider feature | **Hardened** | **Essential; preserve** |
| Sandbox backends | Restrict process capabilities consistently | Seatbelt, bubblewrap or Docker wrap workers and keyless gate evaluation; protected Job/key/proof paths are denied or read-only across provider routes | Provider-independent capability boundary | **Stable / bounded for strict receipts**: strict Jobs refuse `none` before signing; a real Docker test proves the common control boundary, while a public strict Docker Job and live Linux gate proof remain outstanding | **Preserve fail-closed receipt rule; continue backend parity tests** |
| Provider abstraction | Route the same run contract across APIs and CLI harnesses | Provider trait, router, capability registry, concrete adapters and descriptor ingestion | Harness-of-harness portability | **Stable / maturing**: broad coverage, uneven structured-event and steering parity | **Essential; extend via contracts/descriptors, not bespoke surface growth** |
| Structured provider contracts | Replace output scraping with declared event semantics | Pi/Copilot and app-server paths emit/consume typed events; other adapters retain parsing gaps | Makes evidence and control portable rather than terminal-shaped | **Maturing** | **Essential direction; finish parity** |
| Budgets and context | Bound delegated work and reveal consumption | Wall-time, turn and API-spend caps plus spend/context records drive status and stopping | Budget is enforced by the outer harness | **Stable / bounded**: subscription quotas and context visibility vary by provider | **Preserve and extend** |
| Evidence ledgers and protocol | Make every important decision reconstructable | Append-only run, trace, spend, flight and narrative records feed a shared projection | Evidence is a first-class artifact, not a log side effect | **Stable**: Keel centralized the wire vocabulary, persistence policy, readers, writers and checked schemas | **Essential; preserve compatibility and route new formats through the protocol** |
| `RunView` and `JobView` projections | Give surfaces one interpretation of evidence and lifecycle | `RunView` retains rich run facts; `JobView` folds Job events and composes the linked run | Separates control truth from evidence without duplicating either | **Stable / maturing**: legacy adapters are read-only; all surfaces are not yet Job-native | **Essential; complete projection/resolver parity** |
| Two-key completion and verdict | Separate "agent finished" from "work accepted" | A native contained HMAC gate must pass, then a fresh read-only semantic judge must return `achieved`; the supervisor seals a combined receipt | Natural-language meaning is checked without weakening deterministic failure | **Stable / bounded for durable Jobs**; explicitly legacy objects remain deterministic-only | **Core differentiator; benchmark false acceptance/rejection** |
| Snapshots, undo and rewind | Recover from autonomous mistakes without throwing away the whole run | File snapshots and diffs support undo; flight events provide checkpoint-like rewind | Recovery belongs to the outer control plane | **Stable / bounded**: file rollback is real; provider conversation rollback is not | **Preserve; state the boundary clearly** |
| Finish, promotion and library | Make acceptance and reuse explicit | Durable Job promotion revalidates authority, marker, semantic judgment, HMAC and result-tree digest; legacy promotion retains its marker path | The user accepts evidence-backed work, not an opaque session | **Hardened for the Job validator; compatibility split remains** | **Essential; preserve validation before promotion** |
| Attach / Helm | Observe long-running work from the operator's seat | A file-backed TUI reads the same durable state and supports bounded control actions | The UI can disappear without taking the run with it | **Stable / maturing** | **Essential operator surface; preserve and improve responsiveness** |
| Show, status, history and static report | Turn durable evidence into an inspectable artifact | Deterministic summaries and HTML reports render the shared projection | Strong local-first auditability without a service | **Stable / bounded** | **Preserve deterministic core; treat richer rendering as optional** |
| Docs and narration | Explain what happened at human scale | Deterministic docs plus optional provider-generated narrative summarize progress and artifacts | Evidence can be consumed without reading raw event streams | **Maturing / experimental**: several overlapping narrative paths | **Preserve one deterministic path; consolidate and validate model-generated variants** |
| Import | Bring work from other agent harnesses into the same evidence model | Parsers ingest supported transcript/session formats into local run artifacts | A partial cross-tool memory bridge | **Stable / bounded**: deliberately one-way, not shared live state | **Preserve; do not claim the broader need is solved** |
| Chains | Express sequential verified work | New supported chains compile into a durable linear Graph Job verified once at the end; historical `chain run|resume` and unsupported conductor policies remain explicitly legacy | Reuses one scheduler without pretending unsupported hooks/apply policy survived translation | **Maturing**: ordinary creation is Job-scheduled; historical and policy-rich compatibility paths remain | **Dogfood the durable path; preserve or retire legacy behavior explicitly** |
| Plans, fork/merge and review | Coordinate dependent work and reconcile branches | A saved DAG launches child runs, reviews results and merges accepted work; direct orchestration and stored-plan fork force at-end delivery, verify the same-ID Graph parent and can repair that parent after semantic `revise` without rerunning successful leaves | Established graph semantics under one parent lease and receipt | **Maturing**: verified parent completion, bounded parent repair and durable direct launch exist; live interruption drills do not | **Dogfood parent repair; keep compatibility modes honest** |
| Campaigns and reshape | Lift orchestration one level for broad goals | Bounded depth-two sub-orchestrations can recover exact persisted sub-plans; durable direct/guided parent completion revalidates the worst-of roll-up and can repair the merged parent before a two-key receipt | Worst-of roll-up and parent gate prevent child-result laundering | **Experimental / maturing**: durable parent recovery, bounded parent repair and receipt exist; live interruption drills do not | **Freeze depth; dogfood recovery, repair and no-laundering behavior** |
| Seams | Let policy hooks compose without taking over the kernel | Four fixed subprocess seams receive versioned input and produce bounded output; conformance tooling checks them | Extensibility at explicit control points | **Stable / bounded** | **Preserve the fixed model; resist universal hook proliferation** |
| App-server steering | Control a live inner harness through a richer protocol | Codex app-server integration and a provider-neutral inbox support bounded steering/approvals | Moves beyond stdout scraping | **Experimental**, opt-in and Codex-led | **Preserve the neutral pattern; validate before broad surface investment** |
| Learning and self-improve | Mine prior runs and propose changes to DeadReckon itself | Indexing and learning commands derive observations/proposals from artifacts | A self-hosted improvement loop | **Experimental**; weak connection to the original highest-priority needs | **Strong product-decision/deprecation candidate** unless usage and proposal quality justify it |
| Notifications and sleep inhibition | Support unattended operation | Optional notifications and platform sleep handling wrap long runs | Useful operational polish | **Stable / peripheral** | **Keep while cheap; not strategic** |
| Supervisor service operations | Restore local work after the worker shell or supervisor disappears | Explicit managed launchd/systemd definitions pin binary, home and PATH; install/start/status/stop refuse unmanaged conflicts | Machine-level posture is opt-in and inspectable | **Implemented definitions and commands; live cross-platform reboot acceptance outstanding** | **Dogfood before making restart-at-login a default claim** |
| Doctor, setup, update and release trust | Make the binary installable and diagnosable | Environment checks, provider setup, update flow, signing/attestation and packaging support operations | Necessary for trusting a local supervisor binary | **Stable / maturing** | **Add service/containment preflight; otherwise maintain as infrastructure** |

## Durable Job gates are closed; compatibility paths remain

Watchkeeper closes the concrete trust gaps identified by the previous map for
durable Single, Graph and Campaign Jobs created through guided or supported
ordinary direct launches:

- the trusted controller materializes the approved `acceptance.yaml` before
  evaluation;
- keyless `dr-gate evaluate` runs under the backend that actually resolves,
  receives no `GATE_*` inputs, and cannot write proof or Job control files;
- the sandbox runner scrubs inherited gate inputs and reaps the evaluator's
  whole process group, including delayed descendants;
- a private release pipe prevents repository checks from starting until a
  unique per-attempt evaluator record is atomically synced with its outer
  launch, boot and process-start identity;
- cancellation and retry reconcile the outer worker and every nested evaluator
  identity before becoming terminal or launching another attempt; corrupt,
  reused or unverifiable identity stops `LostContainment`;
- a strict resolved backend of `none` refuses before signing material is read;
- only then does childless `dr-gate sign` read the external HMAC key, strictly
  revalidate the evaluation, contract and tamper facts, reconstruct progress
  and tamper evidence, and sign the observed backend;
- version-2 native markers and final receipts use HMAC-SHA-256 with
  constant-time verification;
- key material lives outside the run workspace under an owner-only key store;
- authority, contract, receipt, lifecycle, gate, proof, snapshot and provenance
  control paths are denied or read-only across Seatbelt, bubblewrap, Docker,
  CLI providers and the Codex app-server boundary;
- canonical/symlinked path variants are covered;
- issuer, proof kind, resolved sandbox backend and containment are signed in
  the marker and final receipt;
- a synthetic controller marker is not a native `dr-gate` proof;
- Job policy stores the requested sandbox selector and tool capabilities;
  authority binds that policy by digest but does not claim which backend will
  resolve at runtime;
- trusted Git routing captures and validates the `.git` redirect, run
  worktree, linked-worktree Git directory and common Git directory; provider
  commits and index state are discarded before trusted result commits;
- receipt fields bind optional source and result revisions and deliverable
  source and result tree digests; validation separately enforces merge-aware
  path history, filesystem kind, executable mode and symlink target rather
  than trusting mtime;
- active Git filters and gitlinks fail closed instead of escaping the artifact
  model;
- a verified worktree apply validates every introduced history path and resets
  the target to its pre-delivery revision if final identity checks reject it;
- an uncontained or `none` result cannot become a verified strict receipt.

The second key is semantic. Deterministic success triggers a fresh read-only
provider request over bounded, supervisor-assembled evidence. Only
`achieved` can seal the final receipt. A Single Job can use `revise` for
another bounded worker turn. A Graph or Campaign parent uses `revise` to start
a new fenced, bounded parent-only attempt over the merged result; successful
leaf work is neither rerun nor rewritten. Repeated repair rounds are linked by
attempt, launch, lease and tree identity. `uncertain`, unavailable or malformed
results stop `NEEDS_REVIEW`. Deterministic failure never calls the judge.

This boundary covers durable Jobs, not every compatibility path. Supported
ordinary `run` and `orchestrate`, new chains, stored-plan `fork`, and direct
campaigns, public `extend`, and guided follow-ups enter the same Job scheduler.
Preview, explicit in-place or uncontained work, historical `chain run|resume`,
unsupported conductor-only policies, and chain extension retain process-owned
compatibility semantics. Host
configuration also matters: strict verification needs a real resolved
sandbox. The repository contains hostile-path, forgery and fault tests. Live
adversarial trials across providers and host versions remain operator work. The
real macOS public-command suite proves the two-phase Seatbelt gate, protected
path denial, gate-input scrubbing, residual cleanup and signed observed backend;
it also cancels a held-open evaluator and SIGKILLs the outer launcher, proving
the old group is reaped before cancellation or one bounded retry. An opt-in
real Docker test separately proves the common key, environment, network and
control-path boundary. Live Linux/bubblewrap proof, a public strict Docker Job
with a platform-compatible gate, and a real service-backed reboot remain
outstanding.

## Original unmet needs: current outcome

The original research ranked 25 needs. The table distinguishes implemented primitives from the actual operator outcome.

| # | Unmet need | Current outcome | Assessment |
|---:|---|---|---|
| 1 | Live context and spend visibility | Spend records, caps, context meter and status exist; subscription/model telemetry remains uneven | **Partly met** |
| 2 | Multi-agent worktree coordination | Plans, new chains, stored-plan forks and campaigns coordinate isolated runs under a durable verified parent Job; policy-rich legacy paths and resource leasing remain | **Mostly met; live outcome unvalidated** |
| 3 | Undo for agent changes | Snapshots, diff, undo and file rewind exist | **Strongly met**, within file-state scope |
| 4 | Provenance for generated code | Events, traces, artifacts and lineage are persisted | **Strongly met** |
| 5 | Searchable team memory | Local library, docs and import are searchable; shared team memory and automatic carryover are absent | **Partly met** |
| 6 | Cross-tool state | Multiple tools can be imported and providers share a control vocabulary; state is not bidirectionally live across tools | **Partly met** |
| 7 | Serious operator UI | Helm and reports provide dense terminal/static inspection; no live web/API or desktop control plane | **Partly met** |
| 8 | Observability, evals and root-cause analysis | Flight, traces, verdict and reports exist; automated RCA and cross-run evals do not | **Partly met** |
| 9 | Sandboxed execution | Multiple real backends and isolated codebases exist; strict receipts fail closed when resolution yields `none`, while compatibility runs may still request it | **Mostly met; compatibility/default gap** |
| 10 | Billing guardrails | API spend, turn and wall caps exist; subscription quota semantics and universal usage accounting do not | **Partly met** |
| 11 | Permission controls | Sandbox policies, tool handling and app-server approvals exist; Watchkeeper protects key and authority paths across those routes | **Mostly met; broader provider parity remains** |
| 12 | MCP client/server interoperability | No general MCP server or client surface | **Absent** |
| 13 | Team onboarding | Setup, doctor, provider discovery and contract inference exist | **Implemented; outcome unvalidated** |
| 14 | Structural verification of completion | Durable Single, Graph and Campaign Jobs created through guided or ordinary direct launches require contained deterministic checks plus independent semantic `achieved` and a combined parent receipt | **Strongly met in implementation; live false-decision rates unvalidated** |
| 15 | Discoverable hooks and gates | `def-done`, doctor, four seams and conformance tooling exist | **Strongly met** |
| 16 | Provider routing | Registry, capabilities, descriptors and routing cover several providers | **Mostly met; provider parity remains** |
| 17 | Handoff and continuity | Attach, status and docs help a human resume; public and guided follow-ups now freeze verified parent identity and context into a durable child Job, while first-class handoff export and selective memory carryover remain absent | **Mostly met for local continuation** |
| 18 | Port and environment isolation | Worktrees and process sandboxes isolate files/processes; there is no port/env lease broker | **Partly met** |
| 19 | Governance receipts | Durable Jobs bind approved authority, checks, semantic judgment, optional revisions, result digest and confinement in an HMAC receipt; rules/skill receipts remain absent | **Mostly met; live cross-provider proof remains** |
| 20 | Paid-review continuity | Review runs exist, but no specific paid-review workflow or continuity layer | **Mostly absent / not prioritized** |
| 21 | Local-first operation | Durable file-backed state and static artifacts are foundational | **Strongly met** |
| 22 | Meeting-to-code traceability | No dedicated workflow | **Absent / intentionally out of scope so far** |
| 23 | Prompt and team standards | Skills, contracts and seams provide primitives; rules-as-gate is drafted but not implemented | **Partly met** |
| 24 | Efficiency evaluation | Per-run spend/time/evidence exist; cross-run efficiency analysis and RCA do not | **Partly met** |
| 25 | Agent inventory and run queue | Guided and ordinary Single, Graph and Campaign Jobs—including run follow-ups—are queued, listed, leased and supervised locally; historical chain extension and port/env/resource leasing remain outside it | **Mostly met locally, with resource leasing absent** |

## What is essential

These capabilities form the product's defensible spine and should be preserved, improved and extended:

1. **The verified Job kernel**: immutable authority, append-only lifecycle,
   fenced supervision, typed stop reasons and deliberate promotion.
2. **Executable two-key completion**: definition of done, contained deterministic gate, read-only semantic judge and explainable receipt.
3. **Isolation and recovery**: codebase modes, process sandboxing, snapshots, undo and retained failed evidence.
4. **Provider-neutral outer control**: one contract across direct APIs, CLI harnesses and structured app-server protocols.
5. **Evidence as a protocol**: append-only records, provenance, spend, flight data and composed `RunView`/`JobView` projections.
6. **Operator control without UI ownership**: attach/status/finish surfaces that read durable truth rather than holding it in memory.
7. **A migration path from verified runs to Jobs**: preserve chain and plan
   behavior while deciding the remaining historical chain ownership under one
   scheduler.
8. **Local-first inspectability**: reports, library artifacts and import/export that do not require a hosted service.

The uniqueness is not any individual command. It is the combination: provider-independent delegation + durable evidence + independent acceptance + recoverable promotion.

## What should be treated as cruft or compatibility debt

These are candidates, not automatic deletions. Each should be checked against actual scripts and usage before removal.

| Candidate | Why it is debt | Recommended treatment |
|---|---|---|
| Hidden `acceptance` beside canonical `def-done` | Duplicate name for the same concept | Warn, document `def-done`, remove after a stated window |
| Hidden `materialize` while visible `export` is its alias | Canonical/internal naming is inverted | Make one name canonical, migrate tests/scripts, remove the other |
| `abandon` / `discard` overlap with `cleanup` | Similar destructive lifecycle vocabulary increases operator uncertainty | Keep only a distinct single-run semantic; otherwise fold into cleanup |
| `list --full` compatibility mode that is accepted but ignored | Code and tests preserve behavior with no present meaning | Deprecate directly and remove after compatibility check |
| Legacy aliases such as `--force`, `--all`, `--branch`, `--strategy`, `--budget-cap` | Expands permanent parsing and documentation surface | Publish one migration table, then delete together |
| Reserved no-op pipeline phase 10 `plan` | Implies functionality without executing it | Remove or implement only when a real consumer requires it |
| `learn index --since` accepted but ignored | Misleading surface | Implement real filtering only if learning survives product review; otherwise remove flag with feature |
| Legacy single `run-narrator` path beside newer narration components | Parallel ways to produce the same optional polish | Consolidate behind one deterministic narrative pipeline |
| Stale architecture/version text and placeholder comments | Makes repository evidence contradict the code | Regenerate/trim `AS-BUILT-ARCHITECTURE.md`; remove stale verdict placeholder language |
| Motion/effects and multiple generated-narrative variants | Optional decoration can accumulate latency and maintenance | Keep behind one cheap optional layer; do not let it affect run truth |

There is also structural cruft risk in the breadth of the top-level CLI. Advanced primitives are not themselves cruft, but exposing nearly every internal capability as a peer command makes the product harder to understand. The default five-command flow—`start`, `attach`, `status`, `list`, `finish`—should remain the front door; advanced operations should be grouped into a coherent namespace or progressive-disclosure surface.

## What is half-built or awaiting a product decision

### Engineering work already chosen

- **Graph and Campaign parent repair proof**: both guided shapes can verify,
  repair, receipt and promote the merged parent. A semantic `revise` starts a
  bounded parent-only attempt under the current Job lease. Its intent,
  manifest, candidate and archived marker/judgment form a chained lineage;
  recovery can adopt a fully written candidate without starting a duplicate
  worker. Receipt sealing and `finish` reread that lineage as stable regular
  files and fail closed on identity drift, mutation, removal or symlink
  substitution. Hermetic Graph and Campaign tests cover one and repeated
  revise rounds; live provider and interruption trials remain open.
- **Remaining compatibility parity**: supported new direct and advanced
  execution and run continuation are Job-scheduled. Historical chain
  execution, policy-rich chain modes, explicit in-place/uncontained execution,
  previews, and chain extension remain process-owned and need preservation or
  intentional retirement.
- **Outer-worker crash-window closure**: the supervisor records the prepared
  launch and attempt before spawn. The worker blocks on a private pipe until
  its metadata and `ChildLinked` event are durable. A pre-release crash can
  relaunch the same logical attempt. Post-release recovery also requires a
  valid release acknowledgement tied to that linked launch, plus matching boot
  and process-start identity. Missing or conflicting evidence fails closed.
  Same-ID root mappings and reserved Campaign sub-plan IDs are recoverable
  without replanning.
- **Aggregate advanced budgets**: root planner spend/wall usage is embedded
  before child work, restored after mapping crashes, subtracted from the Job
  policy and divided across children. Typed Graph/Campaign budget exhaustion
  remains terminal after sidecar loss or a supervisor restart.
- **Provider parity**: structured events, context accounting, steering and approvals differ across adapters.
- **Fault injection and measurement**: the repository has deterministic tests
  for protocol corruption, lease races and reclaim, protected paths, receipt
  tampering, promotion refusal and service rendering. The real macOS
  public-command end-to-end test proves the contained two-phase Seatbelt gate,
  and a real Docker test proves the common control boundary. The committed
  credential-free adversarial result records 12 passes and 0 failures. The
  24-row live operator kit records 2 attempted tasks, 22 not run, and 0
  verified. A passive recorder defines evidence and oracles for all 9
  remaining live claims without initiating their destructive actions. Its
  protected `dr-capture` mode authenticates the exact Job, append-only history,
  deterministic evaluation and HMAC publication receipt; operator-supplied
  compatibility captures remain explicitly inconclusive.
  There is no live Linux/bubblewrap or public strict Docker Job result, reboot
  result, or measured false-accept and false-reject rate.

### Product bets needing validation

- **Campaign depth**: useful implementation, but no evidence yet that depth-two sub-orchestration deserves more complexity or concurrency.
- **Generated/live narration**: human-readable summaries matter; multiple model-backed narration routes may not.
- **Codex app-server steering**: promising protocol integration whose provider-neutral inbox abstraction matters more than a Codex-specific surface.
- **Learning/self-improvement**: substantial code without a clear connection to the top operator needs. Require evidence of repeated high-quality proposals or deprecate it.
- **Flight rewind**: valuable as file recovery; do not imply semantic restoration of provider conversation unless that becomes a real protocol feature.

### Important unmet extensions

- Explicit gated `fork <run-id> --prompt ...` from a live or completed run.
- Port, environment and resource leasing above the existing durable local
  single-job queue.
- Cross-machine or hosted scheduling as a separate product decision, not an
  implied property of the local Watchkeeper.
- Rules-as-gate and receipts showing which standards were applied.
- MCP server/client access to start, status, `RunView`, evidence and verdict.
- First-class handoff export and selective memory carryover between runs/tools.
- Cross-run RCA and efficiency comparison built from protocol records.
- A general provider-neutral approval/pause seam.
- Live read-only API/web projection, if terminal and static reports prove insufficient.

## Recommended sequence

1. **Dogfood the durable-Job promise.** Execute the existing 24-row kit across
   its two repositories and providers, recording verified completion,
   recovery, intervention, comprehension time and supervision/judging cost.
2. **Exercise crash and service recovery.** Kill workers and supervisors, remove
   network access, restart the machine, and attempt gate tampering. Close only
   the crash windows demonstrated by those drills.
3. **Resolve the remaining compatibility boundary.** Preserve or intentionally
   retire historical chain execution and extension, preview and uncontained
   behaviors without weakening the Job lifecycle or labelling untrusted work
   verified.
4. **Turn team policy into an acceptance input.** Deliver rules-as-gate on top
   of the existing done contract and combined receipt.
5. **Add resource leasing only after scheduler parity.** Keep cross-machine
   scheduling as a separate decision.
6. **Expose, do not duplicate, the control plane.** Add MCP around the existing
   lifecycle and projections.
7. **Prune before expanding again.** Remove compatibility no-ops and
   consolidate narration; require usage evidence for campaign and learning
   depth.

## Decision rules for future work

A proposed feature belongs in DeadReckon when it strengthens at least one of these patterns:

- makes delegation more bounded;
- makes completion more independent and explainable;
- makes state more durable, portable or reconstructable;
- improves containment or recovery;
- composes verified runs without weakening their invariants;
- gives the operator more control over the same source of truth.

It is suspect when it:

- reproduces a capability that inner harnesses now provide adequately;
- adds another truth model, narrative pipeline or completion path;
- exposes an implementation detail as a permanent top-level command;
- adds orchestration depth before guided Job supervision is proven in live use;
- is polished and tested but cannot be tied to an observed operator need.

## Evidence base and limits

This map was derived from the current CLI and crate graph, implementation
paths, tests, changelog, goal and rider history, [`PRODUCT.md`](../PRODUCT.md),
[`README.md`](../README.md), [`CONCEPTS.md`](CONCEPTS.md),
[`AUDIT-2026-05-11.md`](AUDIT-2026-05-11.md), and
[`AS-BUILT-ARCHITECTURE.md`](AS-BUILT-ARCHITECTURE.md).

The map is intentionally stricter than milestone labels such as “closed” or
“stable”: those labels demonstrate implementation progress, while this
document asks whether the operator outcome is actually met. The real macOS
public-command gate trial proves the contained two-phase Seatbelt path, and the
opt-in Docker trial proves the common container control boundary. Unit and
integration tests and the 24-row dogfood result, with 2 attempted tasks, 22 not
run and 0 verified, do not establish live Linux/bubblewrap or a public strict
Docker Job, successful reboot recovery, live cross-provider behavior, or
false-accept and false-reject rates. Market maturity, frequency of use and
willingness to pay cannot be inferred from this repository and remain
validation questions.
