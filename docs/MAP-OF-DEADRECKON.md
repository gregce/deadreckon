# Map of DeadReckon

Status: repository analysis refreshed 2026-08-02 through Soundings
implementation/hardening commit `00fa040`, role-aware execution teams, strict
durable-Job admission, cumulative wall-cap enforcement, durable provider
progress, Linux sandbox/toolchain hardening and the previously recorded
Watchkeeper adversarial trials.

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
   fenced local supervision, frozen role-specific execution choices and
   isolated result workspaces across guided and ordinary direct execution.
   This separates result changes from the target checkout; it does not claim
   universal host filesystem confinement. Durable Single,
   Graph and Campaign Jobs admit only a sealable deterministic contract, stop
   at attempt-count, cumulative active-attempt wall-time and absolute-deadline
   limits, and prove 2 independent completion decisions before deliberate
   promotion. Preserve and dogfood this.
2. Established graph/campaign conductors, direct advanced commands, reports,
   imports, and narration. Ordinary direct orchestration, new chains,
   stored-plan forks and campaigns now run those conductors under a parent Job
   and verify the merged parent. Preview and explicit in-place/uncontained
   execution remain foreground and untrusted. Historical chain execution and
   mutation refuse at the public boundary; the characterization binary alone
   retains the old process-owned behavior for tests. Public and guided run
   follow-ups use a parent-bound durable Single Job.
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
| Providers | `crates/deadreckon-providers` | Provider trait/router, provider-scoped model catalogs, direct HTTP models, concrete CLIs, descriptor-driven CLIs and Codex app-server support | Makes the control vocabulary portable across changing agent harnesses. |
| Sandbox | `crates/deadreckon-sandbox` | Seatbelt, bubblewrap, Docker and explicit no-sandbox execution, including approved toolchain/runtime mounts | Creates the capability boundary between delegated work and the host. |
| Protocol | `crates/deadreckon-protocol` | Versioned run ledgers plus Job, lease, authority, evaluator/sandbox evidence, semantic judgment, execution evidence, completion receipt, Git delivery and authenticated operator-capture wire types | Separates durable evidence from implementation internals. |

The intended dependency direction is operator surface -> runtime -> provider/sandbox/core -> protocol. Keel moved the persisted ledger types, readers and writers onto that downward dependency and added checked schemas without changing the stored bytes.

The checked JSON-schema set covers ledgers, Job control, sandbox observation,
completion and operator capture. `GitDeliveryIntent` and
`AppliedGitDeliveryReceipt` are typed and authenticated protocol artifacts but
are not yet exported by `all_schemas()` into `docs/schemas`; that is a real
schema-coverage gap, not evidence that delivery is untyped.

### Two execution engines, one outer contract

The provider abstraction deliberately spans two materially different inner loops:

- Direct APIs return one structured action such as `bash`, `write_file`, `reshape` or `done`; the DeadReckon runtime executes it.
- CLI harnesses such as Codex or Claude mutate the working tree themselves; DeadReckon observes them, records evidence, snapshots changes and judges completion afterward.

Provider descriptors turn compatible CLIs into configuration rather than one bespoke Rust implementation per tool. This is a genuinely important abstraction: the stable product is the outer contract, not any vendor's current command-line syntax.

### Execution teams are frozen, provider-scoped inputs

Guided `start` now treats a provider and model as one execution choice. A
review can use one pair for both coder and reviewer or customize them. A full
plan can do the same for planner, default implementor and individual children.
Codex routes can discover visible models from Codex-owned cache state; every
other route, including user descriptors, uses its own descriptor catalog.
Discovery failure falls back to that same provider's descriptor and never
borrows another provider's models.

The exact per-role provider/model pairs are written through the accepted
launch plan and durable driver, then restored on supervisor recovery and
replay. Older persisted Jobs retain a deliberately narrow fallback from their
single global model field. This is a material correction to the old model in
which provider roles were explicit but one model value could leak across them.

### One verified Job is the compositional atom

Guided `start` and ordinary direct execution now prove the stronger atom: one
approved Job, one append-only lifecycle, one same-ID parent result, contained
deterministic checks, one fresh semantic judgment, and one signed receipt.
Before creating the Job, admission rejects contracts with no checks, no
required checks, or only the pre-created working-directory existence check.

Direct `run` creates a durable Single Job. Direct `orchestrate`, stored-plan
`fork`, and new supported chains create durable Graph Jobs. Direct `campaign`
creates a durable Campaign Job. Ordinary Graph plans compose at the end;
durable chain Graphs apply accepted nodes sequentially into an isolated ordered
candidate. Both materialize and verify one same-ID parent result at completion.
A Campaign can recover exact persisted sub-plans and revalidates its worst-of
roll-up. The supervisor then receipts and promotes the same-ID parent result.
Preview and explicit in-place/uncontained execution remain foreground and
untrusted, so they cannot issue a trusted Job receipt. Public historical
`chain run|resume` refuses before mutation or execution. Public `chain extend`
and `chain redo --extend` refuse before mutation and offer an updated durable
schedule. Unsupported policy-rich chain launch also refuses rather than
silently choosing the old conductor. Only the characterization binary retains
that legacy behavior for tests. Public `deadreckon extend` and a follow-up
selected by guided `start` freeze the completed parent state,
promoted-artifact tree and, for a verified Job parent, its receipt, then queue
a durable Single Job. A `LegacyUnowned` parent has no receipt to freeze and is
recorded explicitly as that weaker parent kind.

Guided source admission is now source-true before any provider call or write.
`start --mode review|full-plan --from <dir>` previews the canonical source,
authors a launch-project contract from a bounded dossier of that source, then
freezes tracked and untracked deliverables into a controller-owned approved
copy before queueing the Graph Job. Preview, authority and driver bind the same
digest; children never work from the mutable external path. Draft, critic and
optional redraft use schema-only provider requests under one 120-second default
deadline and reap the full provider process tree on cancellation.

The wall cap is cumulative across active attempt intervals, including an
attempt that spans supervisor or machine restart. At exhaustion the supervisor
reconciles the owned worker, nested evaluator, Campaign sub-plan, repair and
Docker process authorities before recording `WallCap`; inability to prove that
cleanup records `LostContainment`. Provider completion and accounting are
persisted before local Git, snapshot, provenance, documentation or gate
post-processing, so later interruption does not leave a completed provider
turn looking like a turn-zero zombie.

Graph and Campaign parents can now act on semantic `revise` through bounded,
fenced parent-only repair. The next consolidation step is dogfooding that
promise and retiring compatibility modes whose semantics can be represented
truthfully by Jobs.

## Capability portfolio

| Capability | Need and abstracted pattern | How it works | What is distinctive | Maturity | Disposition |
|---|---|---|---|---|---|
| Guided `start` and Course | Turn an underspecified request into a bounded run contract | Intake resolves one canonical source before provider work, then selects an execution team, budget and done conditions; preview, contract, authority and dispatch share that decision, and Course can decompose scope before launch | A friendly entry to a rigorous outer harness without late source contradictions | **Maturing**: source admission and provider/model choice are coherent, but accepted Course pieces influence count while the plan can re-decompose them | **Preserve and align**; keep planning advisory and make accepted decomposition authoritative enough to avoid surprise |
| Execution teams and model catalogs | Keep intelligence choices explicit for every orchestration role | A uniform provider/model pair or custom planner, coder, reviewer, default-child and per-child choices are resolved per provider and frozen into plans/drivers | Recovery and replay use the approved team without cross-provider model leakage | **Maturing**: role freezing/recovery are tested, but live discovery is Codex-cache-only and other/custom routes use static descriptor catalogs | **Preserve the provider-scoped contract; add discovery only where the provider owns a reliable source** |
| Definition-of-done compiler and admission | Make acceptance executable instead of conversational | `def-done` and setup infer or generate the independent gate command; guided authoring keeps files at the launch root while a capped/redacted dossier describes the resolved source, uses strict output schemas under one 120-second default budget, and durable admission atomically refuses empty, optional-only and working-directory-only contracts before a Job exists | Converts real source facts into a portable machine-checkable boundary without giving the authoring provider tools or an unbounded pre-launch turn | **Stable / bounded** | **Essential; extend** with rules-as-gate and clearer explanations |
| Persisted run kernel | Retain work and evidence when a process fails | A run directory stores state, working tree, snapshots, events and evidence; provider outcome/accounting are saved before fallible local post-processing | File-backed facts survive their original terminal and completed provider work cannot remain falsely at turn zero | **Hardened as persistence**, not by itself a relaunch guarantee | **Essential; keep rich run evidence below Job control** |
| Durable Job supervisor | Approve before work, detach safely and stop for typed reasons | Guided and ordinary direct single, review, full-plan, chain, stored-plan fork and campaign launches write immutable authority and one append-only parent Job; a fenced lease supervises the root tree and enforces cumulative active-attempt wall time | Separates process exit from lifecycle truth, gives one durable parent ID and fails closed if the whole owned tree cannot be reconciled | **Stable / bounded**: supported launches have bounded wall/deadline/retry semantics and can earn a verified receipt; live reboot proof remains outstanding | **Highest-priority dogfood; close live recovery proof** |
| Lifecycle control | Start, observe, cancel, deliver and undo Jobs coherently | The shared resolver includes Jobs; `list` suppresses backing duplicates; status/attach/kill/finish use typed outcome and stop reason; apply/export and undo append post-terminal delivery events | Ordinary operations and later delivery read one persisted control truth without rewriting terminal verification | **Stable / bounded**: the cross-kind journey, Job resolver and post-terminal reducer are tested | **Preserve one resolver and one Job event truth** |
| Codebase isolation | Separate intended result changes and make promotion deliberate | Worktree, copy, fresh-repo and explicit in-place modes separate the result workspace from the user's target; host access is governed separately by the selected sandbox | Result isolation is part of the run contract, not a provider feature or a claim of universal host confinement | **Hardened** | **Essential; preserve** |
| Sandbox backends | Protect the strict Job control plane across different host mechanisms | Seatbelt, bubblewrap or Docker wrap workers and keyless gate evaluation; Linux bubblewrap reconstructs private-temp paths and mounts approved loaders, `PATH` entries and named runtime homes read-only | Provider-independent receipt/key/control confinement without making approved tools disappear | **Stable / bounded for the strict control boundary on tested backends; maturing for general filesystem/network and platform parity**: Seatbelt is targeted-deny, and named-host network lists are approval policy rather than a proxy-enforced domain boundary | **Preserve fail-closed receipt rule; continue backend parity tests** |
| Provider abstraction | Route the same run contract across APIs and CLI harnesses | Provider trait, router, capability registry, provider-scoped model resolution, concrete adapters and descriptor ingestion | Harness-of-harness portability | **Stable / maturing**: broad coverage, uneven structured-event, accounting, steering and approval parity | **Essential; extend via contracts/descriptors, not bespoke surface growth** |
| Structured provider contracts | Replace output scraping with declared event semantics | Codex exec, Claude Code, Pi, Copilot and Codex app-server have typed event/usage/session paths; malformed streams degrade with explicit caveats, while Gemini/OpenCode retain gaps | Makes evidence and control portable rather than terminal-shaped | **Maturing** | **Essential direction; finish parity** |
| Budgets and context | Bound delegated work and reveal consumption | Exact positive wall caps, cumulative active-attempt accounting, one absolute deadline across every durable creation route, turn/API-spend caps and spend/context records drive typed stopping | The outer harness cuts in-flight work, carries elapsed attempt time across recovery and reconciles nested process authorities before terminal state | **Stable / bounded**: subscription quotas and context visibility vary by provider | **Preserve and extend** |
| Evidence ledgers and protocol | Make every important decision reconstructable | Append-only run, trace, spend, flight and narrative records feed a shared projection | Evidence is a first-class artifact, not a log side effect | **Stable**: Keel centralized the wire vocabulary, persistence policy, readers, writers and checked schemas | **Essential; preserve compatibility and route new formats through the protocol** |
| `RunView` and `JobView` projections | Give surfaces one interpretation of evidence and lifecycle | `RunView` retains rich run facts; `JobView` folds Job events, composes the linked run and revalidates a terminal receipt against its recorded digest, attempt, run and outer launch | A terminal Verified event stays immutable while stale or tampered proof is surfaced through `verified_receipt_error`, not presented as valid | **Stable / maturing**: completed run-ledger parsing is less fail-closed and all surfaces are not yet Job-native | **Essential; complete projection/resolver parity** |
| Two-key completion and verdict | Separate "agent finished" from "work accepted" | A native contained HMAC gate must pass, then a fresh read-only semantic judge must return `achieved`; the supervisor seals a combined receipt | Natural-language meaning is checked without weakening deterministic failure | **Stable / bounded for durable Jobs**; explicitly legacy objects remain deterministic-only | **Core differentiator; benchmark false acceptance/rejection** |
| Snapshots, undo and rewind | Recover from autonomous mistakes without throwing away the whole run | Recoverable-file snapshots and diffs support undo; flight events provide checkpoint-like rewind; rebuildable roots such as SwiftPM `.build` are omitted from snapshots, recoverable/source copies and changed-file/provenance inventories | Recovery belongs to the outer control plane without preserving disposable tool output as product state | **Stable / bounded**: file rollback is real; provider conversation rollback is not | **Preserve; state the boundary clearly** |
| Finish, promotion and library | Make acceptance and reuse explicit | Durable Job promotion revalidates authority, marker, semantic judgment, HMAC and result-tree digest; Git delivery seals an HMAC intent before mutation and an applied receipt after re-proving the exact after-state | The user accepts evidence-backed work, and apply/undo cannot be confused with the earlier verification decision | **Hardened for the Job validator and authenticated Git delivery; compatibility split remains** | **Essential; preserve validation before promotion and delivery** |
| Attach TUI | Observe long-running work from the operator's seat | Activity, narrative and split views read file-backed Job/run/plan-child/plan/chain/campaign state; visual modes cover architecture, agents, files and evidence, and plain/JSON/why variants detach without cancelling work | The UI can disappear without taking the run with it | **Stable / maturing**: its inline steer cannot target a durable Job backing run | **Essential operator surface; preserve and improve responsiveness** |
| Status, show, history, verdict and report | Turn durable evidence into inspectable answers | `status` gives the current next action; `show` exposes raw legacy detail but is status-like for a Job; `history grep` searches JSONL; `verdict` rechecks current evidence; `report` renders full Job/run Markdown, HTML or JSON | Distinct local-first views serve navigation, forensic search, current re-verification and review packets | **Stable / bounded**, with uneven Job-native depth between commands | **Preserve deterministic core; make the distinctions clearer in help** |
| Docs and narration | Explain what happened at human scale | Deterministic docs plus optional provider-generated narrative summarize progress and artifacts | Evidence can be consumed without reading raw event streams | **Maturing / experimental**: several overlapping narrative paths | **Preserve one deterministic path; consolidate and validate model-generated variants** |
| Import | Bring work from other agent harnesses into the same evidence model | Descriptor-driven Codex, Claude, Pi and Copilot import plus Cursor SQLite ingestion create read-only local run artifacts | A partial cross-tool memory bridge | **Stable / bounded**: deliberately one-way, not shared live state | **Preserve; do not claim the broader need is solved** |
| Chains | Express sequential verified work | New supported chains freeze approved hook bytes and a typed undo policy into an adapter, then compile to a durable linear Graph Job; the receipt binds the ordered candidate-manifest digest and, when present, candidate-application and hook-event ledgers; unsupported policies and historical mutation/execution refuse publicly | Final-tree equivalence cannot hide a different ordered execution or hook history | **Maturing**: ordinary creation preserves supported hooks under Job scheduling; stored historical state is inspectable and legacy behavior is characterization-only | **Dogfood the durable path; decide the stored-state and characterization retirement policy** |
| Plans, fork/merge and review | Coordinate dependent work and reconcile branches | Public `fork` compiles a pending unowned Plan into a durable Graph Job; guided review/full-plan can freeze `--from` into a digest-checked Job-local approved copy; the driver launches/reviews children, merges accepted work, verifies the same-ID parent and can repair only that parent after semantic `revise` | Established graph semantics under one parent lease and receipt without rerunning successful leaves or depending on a mutable operator source | **Maturing**: approved-copy isolation and driver-owned merge are real, while public standalone `merge` is refusal-only and live interruption drills do not exist | **Dogfood parent repair; remove or accurately relabel the public merge surface** |
| Campaigns and reshape | Lift orchestration one level for broad goals | Bounded depth-two sub-orchestrations recover exact persisted sub-plans and revalidate worst-of roll-up; accepted inert `reshape` proposals schedule a durable Graph with the accepted decomposition | Parent verification prevents child-result laundering while reshape can promote one run into explicit orchestration | **Experimental / maturing**: durable recovery/repair exists, public `campaign repair` is refusal-only, and live interruption drills do not | **Freeze depth; dogfood recovery, repair and no-laundering behavior** |
| Seams | Let policy hooks compose without taking over the kernel | Four fixed subprocess seams receive versioned input and produce bounded output; conformance tooling checks them | Extensibility at explicit control points | **Stable / bounded** | **Preserve the fixed model; resist universal hook proliferation** |
| App-server steering | Control a live inner harness through a richer protocol | A durable provider-neutral steer inbox tracks pending/delivered entries; Codex app-server polls mid-turn, uses expected-turn preconditions, answers capability approvals, interrupts before kill and can degrade to Codex exec with a caveat | Moves beyond stdout scraping without dropping or duplicating a stale steer | **Experimental**, opt-in and publicly hard-coded to the Codex server route | **Preserve the neutral inbox pattern; validate before broad surface investment** |
| Learning and self-improve | Mine prior runs and propose changes to DeadReckon itself | Deterministic redacted indexing/report/export/import feeds provider proposals; improvement candidates run in isolation, and PR creation remains an explicit gated action | A self-hosted improvement loop with a redacted evidence boundary | **Experimental**; weak connection to the original highest-priority needs and `learn index --since` is still a no-op | **Strong product-decision/deprecation candidate** unless usage and proposal quality justify it |
| Notifications and sleep inhibition | Support unattended operation | Optional notifications and platform sleep handling wrap long runs | Useful operational polish | **Stable / peripheral** | **Keep while cheap; not strategic** |
| Supervisor service operations | Restore local work after the worker shell or supervisor disappears | `setup --supervisor` installs and starts an identity-bound launchd/systemd user service; real approved `start` requires a current active definition and live boot/PID/start-identity checkpoint | Machine-level posture is explicit, inspectable and a launch prerequisite | **Implemented and hermetically exercised; live cross-platform reboot acceptance outstanding** | **Dogfood the real reboot path before claiming machine recovery** |
| Doctor, setup, update and release trust | Make the binary installable and diagnosable | Environment checks, full local binary/version/channel inventory, supervisor readiness, conservative receipt/service repair, update flow, macOS signing/notarization, checksums/SBOM/attestations and packaging operations | Necessary for trusting a local supervisor binary without letting one install silently replace another | **Stable / maturing**: npm publication/provenance and Windows Authenticode remain explicitly deferred | **Dogfood repair across release channels; maintain as infrastructure** |

## Current command surface

Default help highlights the five-command typical flow—`start`, `attach`,
`status`, `list`, and `finish`—while also exposing `try`, setup/health and
selected control commands. The full `help-all` catalog is much broader:

| Surface | Commands | Current truth |
|---|---|---|
| Front door and catalog | `try`, `start`, `attach`, `status`, `list`, `finish`, `help-all` | `try` is a keyless local proof, not a trusted Job receipt; `help-all` reveals advanced and compatibility surfaces. |
| Contract, setup and provider discovery | `init`, `config`, `completion`, `doctor`, `detect`, `providers`, `models`, `seams`, `def-done`, `update` | `models` is provider-scoped; hidden `acceptance` is the older done-criteria surface. |
| Durable launch and control | `run`, `orchestrate`, `campaign`, `chain`, `supervisor`, `steer`, `kill`, `extend` | Supported non-preview launches enter Jobs except explicit in-place/uncontained `run`; `steer` is Codex app-server-only and does not target a Job backing run. |
| Orchestration building blocks | `plan`, `fork`, `merge`, `reshape`, `campaign repair` | `plan`, public `fork`, and accepted reshape are real; merge and campaign repair execute only inside the current Job driver, so their public forms refuse. |
| Delivery and recovery | `library`, `cleanup`, `undo`, `rewind`, `apply`, `export`, `abandon`, `resume` | `finish` is the trusted front door. `resume`/`continue` is public refusal-only; trusted supervisor recovery is internal. Several older verbs remain hidden or alias-led. |
| Inspection, docs and import | `report`, `verdict`, `history`, `show`, `doc`, `import` | These are read-only or artifact-writing views/imports; they do not alter Job completion truth. |
| Learning and self-improvement | `learn`, `improve` | Implemented but experimental and not part of the primary product promise. |

This inventory is descriptive, not an endorsement of every public name. The
generated help still over-promises standalone `resume`, `merge`,
`campaign repair`, and historical chain-extension behavior that the public
boundary correctly refuses. That help/behavior mismatch is compatibility debt.

## Durable Job gates are closed; refused legacy routes and foreground escapes remain

Watchkeeper closes the concrete trust gaps identified by the previous map for
durable Single, Graph and Campaign Jobs created through guided or supported
ordinary direct launches:

- before any Job state is admitted, the trusted controller rejects an empty
  contract, a contract with no required checks, or a contract whose only proof
  is that the already-created working directory exists;
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
Preview and explicit in-place or uncontained work remain foreground, untrusted
escape hatches. Public historical `chain run|resume` refuses before state
mutation or execution. Public `chain extend` and `chain redo --extend` refuse
before mutation while offering the updated schedule as a durable launch.
Unsupported conductor-only policies refuse before Job creation, planning, or
legacy execution. The characterization binary alone retains the old conductor
and mutation behavior for tests. Host configuration also matters: strict
verification needs a real resolved sandbox. The repository contains
hostile-path, forgery and fault tests. Live adversarial trials across providers
and host versions remain operator work. The real macOS public-command suite
proves the two-phase Seatbelt gate, protected
path denial, gate-input scrubbing, residual cleanup and signed observed backend;
it also cancels a held-open evaluator and SIGKILLs the outer launcher, proving
the old group is reaped before cancellation or one bounded retry. An opt-in
real Docker test separately proves the common key, environment, network and
control-path boundary. Three public strict Docker Job tests using a static
Linux evaluator sidecar also pass on macOS arm64: deterministic completion
followed by semantic `NEEDS_REVIEW`, operator cancellation without retry or
receipt, and worker `SIGKILL` cleanup before exactly one bounded retry. Those
results are bound to clean source `a0d262d` by evidence commit `e1d0825`; they
do not automatically cover the later execution-team, supervisor or Linux
sandbox changes. Ubuntu CI now runs real bubblewrap tool/private-temp tests,
hostile read-only provider checks and the public smoke Job's refusal to issue a
trusted receipt. The full live positive strict-Job claim—protected signing
material plus a valid receipt bound to bubblewrap—and a real service-backed
reboot remain outstanding.
The network-loss recorder is no longer structurally incapable of passing: it
signs the registry-derived worker route and endpoint, records a strict
reachable/unreachable/reachable transition for one durably linked attempt, and
requires that attempt's exact stop and retry-or-approved-terminal lineage.
That is implementation evidence only; the host/provider drill itself remains
unrun and therefore unproven.
The Campaign interruption recorder is also pass-capable for a deliberately
narrow claim. It binds one prepared/released/linked sub-Plan process authority,
requires exactly one later adoption of that same launch under a newer fenced
Job lease, rejects any second launch fact or reopened completed task, and then
requires recovery of the same Plan. It does not broaden that evidence into a
global exactly-once claim, and the live provider interruption remains unrun.

## Original unmet needs: current outcome

The original research ranked 25 needs. The table distinguishes implemented primitives from the actual operator outcome.

| # | Unmet need | Current outcome | Assessment |
|---:|---|---|---|
| 1 | Live context and spend visibility | Spend records, caps, context meter, provider-scoped model catalogs and status exist; subscription/context telemetry remains uneven | **Partly met** |
| 2 | Multi-agent worktree coordination | Plans, supported new chains, stored-plan forks and campaigns coordinate isolated runs under a durable verified parent Job; unsupported policy-rich chain launch now refuses, while resource leasing remains | **Mostly met; live outcome unvalidated** |
| 3 | Undo for agent changes | Recoverable source/control/evidence snapshots, diff, receipt-bound delivery undo and hash-guarded rewind exist; rebuildable output is intentionally discarded | **Strongly met**, within recoverable file-state scope |
| 4 | Provenance for generated code | Events, traces, artifacts and lineage are persisted | **Strongly met** |
| 5 | Searchable team memory | Local library, docs and import are searchable; shared team memory and automatic carryover are absent | **Partly met** |
| 6 | Cross-tool state | Multiple tools can be imported and providers share a control vocabulary; state is not bidirectionally live across tools | **Partly met** |
| 7 | Serious operator UI | The Attach TUI and reports provide dense terminal/static inspection; no live web/API or desktop control plane | **Partly met** |
| 8 | Observability, evals and root-cause analysis | Flight, traces, verdict and reports exist; automated RCA and cross-run evals do not | **Partly met** |
| 9 | Sandboxed execution | Multiple real backends and result workspaces exist; strict receipts fail closed on `none`; Linux CI executes bubblewrap, but Seatbelt is targeted-deny and backend filesystem/network/Windows semantics remain uneven | **Mostly met for the strict control boundary; broader parity is maturing** |
| 10 | Billing guardrails | API spend, turn, absolute-deadline and cumulative active-attempt wall caps exist; subscription quota semantics and universal usage accounting do not | **Partly met** |
| 11 | Permission controls | Sandbox policies, tool handling and app-server approvals exist; Watchkeeper protects key/authority paths, but named-host network lists are approval logic rather than proxy-enforced domain filtering | **Mostly met for Job control; broader capability parity remains** |
| 12 | MCP client/server interoperability | No general MCP server or client surface | **Absent** |
| 13 | Team onboarding | Setup, doctor, provider/model discovery, unified execution-team selection and contract inference exist | **Implemented; outcome unvalidated** |
| 14 | Structural verification of completion | Durable Single, Graph and Campaign Jobs created through guided or ordinary direct launches require contained deterministic checks plus independent semantic `achieved` and a combined parent receipt | **Strongly met in implementation; live false-decision rates unvalidated** |
| 15 | Discoverable hooks and gates | `def-done`, doctor, four seams and conformance tooling exist | **Strongly met** |
| 16 | Provider routing | Registry, capabilities, descriptors, provider-scoped model catalogs and per-role execution teams cover several providers | **Mostly met; event/accounting/steering parity remains** |
| 17 | Handoff and continuity | Attach, status and docs help a human re-enter context; public/guided follow-ups freeze parent state, library tree, bounded context and a verified receipt when the parent has one into a durable child Job; first-class handoff export/selective carryover remain absent | **Mostly met for local continuation** |
| 18 | Port and environment isolation | Worktrees and process sandboxes isolate files/processes; there is no port/env lease broker | **Partly met** |
| 19 | Governance receipts | Durable Jobs bind approved authority, checks, semantic judgment, optional revisions, result/confinement and ordered chain execution in HMAC evidence; Git delivery has authenticated intent/applied receipts, while rules/skill receipts remain absent | **Mostly met; live cross-provider proof remains** |
| 20 | Paid-review continuity | Review runs exist, but no specific paid-review workflow or continuity layer | **Mostly absent / not prioritized** |
| 21 | Local-first operation | Durable file-backed state and static artifacts are foundational | **Strongly met** |
| 22 | Meeting-to-code traceability | No dedicated workflow | **Absent / intentionally out of scope so far** |
| 23 | Prompt and team standards | Skills, contracts and seams provide primitives; rules-as-gate is drafted but not implemented | **Partly met** |
| 24 | Efficiency evaluation | Per-run spend/time/evidence exist; cross-run efficiency analysis and RCA do not | **Partly met** |
| 25 | Agent inventory and run queue | Guided and ordinary Single, Graph and Campaign Jobs—including run follow-ups—are queued, listed, leased and supervised locally; public historical chain mutation refuses and offers a durable schedule, while port/env/resource leasing remains absent | **Mostly met locally, with resource leasing absent** |

## What is essential

These capabilities form the product's defensible spine and should be preserved, improved and extended:

1. **The verified Job kernel**: immutable authority, append-only lifecycle,
   fenced supervision, cumulative limits, typed stop reasons and deliberate
   promotion/delivery.
2. **Executable two-key completion**: definition of done, contained deterministic gate, read-only semantic judge and explainable receipt.
3. **Isolation and recovery**: codebase modes, process sandboxing, snapshots, undo and retained failed evidence.
4. **Provider-neutral outer control**: one contract across direct APIs, CLI
   harnesses and structured app-server protocols, with provider/model choices
   frozen per role.
5. **Evidence as a protocol**: append-only records, provenance, spend, flight
   data, delivery records and receipt-revalidating `RunView`/`JobView`
   projections.
6. **Operator control without UI ownership**: attach/status/finish surfaces that read durable truth rather than holding it in memory.
7. **A migration path from verified runs to Jobs**: keep new supported chain
   creation on the Job scheduler, retain read-only access to historical chain
   state, and turn refused legacy execution/mutation into explicit durable
   schedules.
8. **Local-first inspectability**: reports, library artifacts and import/export that do not require a hosted service.

The uniqueness is not any individual command. It is the combination: provider-independent delegation + durable evidence + independent acceptance + recoverable promotion.

## What should be treated as cruft or compatibility debt

These are candidates, not automatic deletions. Each should be checked against actual scripts and usage before removal.

| Candidate | Why it is debt | Recommended treatment |
|---|---|---|
| Hidden `acceptance` beside canonical `def-done` | Duplicate name for the same concept | Warn, document `def-done`, remove after a stated window |
| Hidden `materialize` while visible `export` is its alias | Canonical/internal naming is inverted | Make one name canonical, migrate tests/scripts, remove the other |
| `abandon` / `discard` overlap with `cleanup` | Similar destructive lifecycle vocabulary increases operator uncertainty | Keep only a distinct single-run semantic; otherwise fold into cleanup |
| Public `resume`, standalone `merge`, and `campaign repair` help for refusal-only routes | The catalog advertises operator mutations that only a trusted Job driver may perform | Retire the public command or make help state the refusal and point to `start`/`fork`/internal recovery |
| Historical chain-extension help beside a refusing boundary | `chain extend` and `redo --extend` copy still imply mutation even though the safe public behavior is to print a durable schedule | Describe the migration schedule, not the retired mutation |
| Wire name `JobShape::LegacyCampaign` for every current Campaign Job | A current product concept is serialized under a compatibility name | Preserve decoding compatibility but introduce a current discriminator/versioned migration before legacy retirement |
| `list --full` compatibility mode that is accepted but ignored | Code and tests preserve behavior with no present meaning | Deprecate directly and remove after compatibility check |
| Legacy aliases such as `--force`, `--all`, `--branch`, `--strategy`, `--budget-cap` | Expands permanent parsing and documentation surface | Publish one migration table, then delete together |
| Reserved no-op pipeline phase 10 `plan` | Implies functionality without executing it | Remove or implement only when a real consumer requires it |
| `learn index --since` accepted but ignored | Misleading surface | Implement real filtering only if learning survives product review; otherwise remove flag with feature |
| Legacy single `run-narrator` path beside newer narration components | Parallel ways to produce the same optional polish | Consolidate behind one deterministic narrative pipeline |
| Stale architecture/version text and placeholder comments | Makes repository evidence contradict the code | Regenerate/trim `AS-BUILT-ARCHITECTURE.md`; remove stale verdict placeholder language |
| Motion/effects and multiple generated-narrative variants | Optional decoration can accumulate latency and maintenance | Keep behind one cheap optional layer; do not let it affect run truth |

There is also structural cruft risk in the breadth of `help-all` and the
top-level parser. Default help highlights the five-command flow and a bounded
set of nearby setup/control commands, but advanced, internal and refusal-only
capabilities still appear as peers in the full catalog. Keep that progressive
disclosure and group or remove the remainder instead of letting it become an
archive of obsolete names.

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
  execution and mutation, including policy-rich launch, now refuse at the
  public boundary; only the characterization binary retains their old
  behavior for tests. Public `resume`, standalone `merge` and
  `campaign repair` also refuse because only the trusted supervisor/Job driver
  may perform those transitions, although help still advertises them. Stored
  chain-state, refused-command and characterization retirement still need a
  decision. Explicit in-place/uncontained execution and previews remain
  foreground, untrusted escape hatches.
- **Outer-worker crash-window closure**: the supervisor records the prepared
  launch and attempt before spawn. The worker blocks on a private pipe until
  its metadata and `ChildLinked` event are durable. A pre-release crash can
  relaunch the same logical attempt. Post-release recovery also requires a
  valid release acknowledgement tied to that linked launch, plus matching boot
  and process-start identity. Missing or conflicting evidence fails closed.
  Same-ID root mappings and reserved Campaign sub-plan IDs are recoverable
  without replanning. Recovery also restores the frozen root coder model unless
  an individual piece carries an explicit override.
- **Aggregate advanced budgets**: root planner spend/wall usage is embedded
  before child work, restored after mapping crashes, subtracted from the Job
  policy and divided across children. Typed Graph/Campaign budget exhaustion
  remains terminal after sidecar loss or a supervisor restart.
- **Protocol schema coverage**: Git delivery intent and applied-receipt types
  are real authenticated protocol artifacts, but the checked `all_schemas()`
  export and `docs/schemas` directory do not yet include them.
- **Provider parity**: provider-scoped model selection is coherent, but
  structured events, context accounting, steering and approvals still differ
  across adapters.
- **Fault injection and measurement**: the repository has deterministic tests
  for protocol corruption, lease races and reclaim, protected paths, receipt
  tampering, promotion refusal and service rendering. The real macOS
  public-command end-to-end test proves the contained two-phase Seatbelt gate,
  and real Docker tests prove the common control boundary plus public strict
  Job completion, cancellation and one bounded worker-death recovery on macOS
  arm64. The committed
  credential-free adversarial result records 13 passes and 0 failures. The
  24-row live operator kit records 2 attempted tasks, 22 not run, and 0
  verified. A passive recorder defines evidence and oracles for all 9 current
  live claims without initiating their destructive actions. The ninth keeps
  the hostile live Docker/provider/receipt claim separate from the narrower
  credential-free Docker lifecycle proof. Its
  protected `dr-capture` mode authenticates the exact Job, append-only history,
  deterministic evaluation and HMAC publication receipt; operator-supplied
  compatibility captures remain explicitly inconclusive. Trials sign exact
  allowed terminal outcome/reason pairs before the fault. Verified outcomes
  still require the normal `CompletionReceipt`; an approved non-Verified
  outcome requires distinct signed terminal-history lineage and the absence of
  a completion receipt. The public strict
  Docker results are bound to the clean committed source named by that result
  artifact. Ubuntu CI now supplies real bubblewrap execution and negative
  smoke-receipt coverage, but there is no recorded live positive strict-Job
  bubblewrap receipt, reboot result, or measured false-accept and false-reject
  rate.

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

1. **Dogfood the durable-Job promise at current HEAD.** Execute the existing
   24-row kit across its two repositories and providers, recording verified
   completion, recovery, intervention, comprehension time and
   supervision/judging cost. Refresh older clean-source evidence rather than
   treating it as proof of subsequent supervisor and sandbox changes.
2. **Exercise crash and service recovery.** Kill workers and supervisors, remove
   network access, restart the machine, attempt gate tampering, and complete a
   positive strict-Job receipt on Linux/bubblewrap. Close only the crash
   windows demonstrated by those drills.
3. **Finish the compatibility migration.** The public historical chain
   execution and mutation boundary is closed. Retire or relabel public
   `resume`, standalone `merge`, `campaign repair`, stale chain mutation help,
   the characterization-only conductor and stored legacy schema; decide
   whether printed migration schedules need a first-class import command. Keep
   preview and explicit in-place/uncontained behavior foreground and untrusted
   without weakening the Job lifecycle.
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

This map was derived from the CLI and crate graph at `bb594a3`, implementation
paths, tests, changelog, goal and rider history, [`PRODUCT.md`](../PRODUCT.md),
[`README.md`](../README.md), [`CONCEPTS.md`](CONCEPTS.md),
[`AUDIT-2026-05-11.md`](AUDIT-2026-05-11.md), and
[`AS-BUILT-ARCHITECTURE.md`](AS-BUILT-ARCHITECTURE.md).

The map is intentionally stricter than milestone labels such as “closed” or
“stable”: those labels demonstrate implementation progress, while this
document asks whether the operator outcome is actually met. The real macOS
public-command gate trial proves the contained two-phase Seatbelt path. Opt-in
Docker trials prove the common container control boundary and, on macOS arm64,
public strict-Job completion, cancellation and worker-death recovery. The
Docker results are recorded against clean source `a0d262d` by evidence commit
`e1d0825`, before the later execution-team, supervisor and Linux sandbox
commits. Ubuntu CI now establishes real bubblewrap command/toolchain execution
and the negative public smoke-receipt boundary. Unit and integration tests and
the 24-row dogfood result, with 2 attempted tasks, 22 not run and 0 verified,
still do not establish a positive live strict-Job receipt on Linux/bubblewrap,
successful reboot recovery, live current cross-provider behavior, or
false-accept and false-reject rates. Market maturity, frequency of use and
willingness to pay cannot be inferred from this repository and remain
validation questions.
