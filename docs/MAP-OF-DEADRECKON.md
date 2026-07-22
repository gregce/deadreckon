# Map of DeadReckon

Status: repository analysis updated 2026-07-22, on `main` at `04b3084`, including the completed Keel protocol migration.

## Executive map

DeadReckon is not primarily another coding agent. It is an outer control plane for coding agents and agent harnesses. The inner harness proposes and performs work; DeadReckon owns the durable run, isolation, budgets, evidence, independent completion decision, recovery, and promotion.

Its irreducible pattern is:

```text
goal + definition of done
          |
          v
isolated, durable run -----> provider / inner harness does work
          |                              |
          +<---- events, spend, diffs, snapshots, provenance
          |
          v
independent gate and verdict
          |
          +---- fail: retain evidence, continue / reshape / recover
          |
          +---- pass: promote deliberately into the user's codebase
```

That is the essential product. The strongest reason for it to exist is the gap between *an agent saying it is done* and *an operator being able to trust, inspect, recover, and accept the result*. This matches the original unmet-needs research: predictable delegation needed operational infrastructure more than another transcript or chat surface.

The repository now contains three different things which should not be judged alike:

1. A differentiated execution kernel: runs, contracts, isolation, evidence, gates, promotion, recovery and a provider-neutral vocabulary. Preserve and harden this.
2. Useful compositions over that kernel: guided start, attach, plans, chains, campaigns, reports, imports and narration. Preserve the patterns, but validate and simplify some surfaces.
3. Compatibility layers, speculative depth and incomplete bets. Deprecate or quarantine these rather than allowing them to obscure the kernel.

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
| Core | `crates/deadreckon-core` | Paths, state, locks, artifacts, snapshots, gates, promotion, plans, chains, campaigns and `RunView` | Durable source of truth and most of the product's invariants. |
| Providers | `crates/deadreckon-providers` | Provider trait/router, direct HTTP models, concrete CLIs, descriptor-driven CLIs and Codex app-server support | Makes the control vocabulary portable across changing agent harnesses. |
| Sandbox | `crates/deadreckon-sandbox` | Seatbelt, bubblewrap, Docker and explicit no-sandbox execution | Creates the capability boundary between delegated work and the host. |
| Protocol | `crates/deadreckon-protocol` | Versioned persisted event, ledger, trace, spend and snapshot wire types | Separates durable evidence from implementation internals; Keel established this as the stable persistence vocabulary. |

The intended dependency direction is operator surface -> runtime -> provider/sandbox/core -> protocol. Keel moved the persisted ledger types, readers and writers onto that downward dependency and added checked schemas without changing the stored bytes.

### Two execution engines, one outer contract

The provider abstraction deliberately spans two materially different inner loops:

- Direct APIs return one structured action such as `bash`, `write_file`, `reshape` or `done`; the DeadReckon runtime executes it.
- CLI harnesses such as Codex or Claude mutate the working tree themselves; DeadReckon observes them, records evidence, snapshots changes and judges completion afterward.

Provider descriptors turn compatible CLIs into configuration rather than one bespoke Rust implementation per tool. This is a genuinely important abstraction: the stable product is the outer contract, not any vendor's current command-line syntax.

### One verified run is the compositional atom

Higher-level features are safest when they compose the same verified run primitive:

- A chain sequences runs.
- A plan creates a dependency graph of runs and merges accepted children.
- A campaign launches bounded sub-orchestrations.
- Review and repair create another run instead of bypassing the contract.

This is the right reusable pattern. New orchestration should strengthen it rather than introduce a second way to decide that work is complete.

## Capability portfolio

| Capability | Need and abstracted pattern | How it works | What is distinctive | Maturity | Disposition |
|---|---|---|---|---|---|
| Guided `start` and Course | Turn an underspecified request into a bounded run contract | Interactive/non-interactive intake selects codebase, provider, budget and done conditions; Course can decompose scope before launch | A friendly entry to a rigorous outer harness | **Maturing**: strong surface, but accepted Course pieces influence count while the plan can re-decompose them | **Preserve and align**; keep planning advisory and make accepted decomposition authoritative enough to avoid surprise |
| Definition-of-done compiler | Make acceptance executable instead of conversational | `def-done` and setup infer or generate the independent gate command and persist the contract | Converts repository conventions into a machine-checkable boundary | **Stable / bounded** | **Essential; extend** with rules-as-gate and clearer explanations |
| Durable run kernel | Let an operator walk away and resume without losing truth | A run directory stores state, lock, working tree, snapshots, events, evidence, nonce and lifecycle status | File-backed reality survives terminal and provider failure | **Hardened** | **Essential; preserve and simplify access to it** |
| Lifecycle control | Start, observe, cancel, resume, extend and clean runs coherently | Locks and explicit transitions prevent conflicting owners; commands operate on durable run state | One vocabulary across providers | **Hardened**, with some command overlap | **Essential; preserve semantics, consolidate verbs** |
| Codebase isolation | Contain changes and make promotion deliberate | Worktree, copy, fresh-repo and in-place modes separate the agent's workspace from the user's target | Isolation is part of the run contract, not a provider feature | **Hardened** | **Essential; preserve** |
| Sandbox backends | Restrict process capabilities consistently | Seatbelt, bubblewrap or Docker wrap commands; `none` is an explicit fallback | Provider-independent capability boundary | **Maturing**: backend parity differs and automatic fallback to `none` weakens the default guarantee | **Essential; harden fail-open policy and parity** |
| Provider abstraction | Route the same run contract across APIs and CLI harnesses | Provider trait, router, capability registry, concrete adapters and descriptor ingestion | Harness-of-harness portability | **Stable / maturing**: broad coverage, uneven structured-event and steering parity | **Essential; extend via contracts/descriptors, not bespoke surface growth** |
| Structured provider contracts | Replace output scraping with declared event semantics | Pi/Copilot and app-server paths emit/consume typed events; other adapters retain parsing gaps | Makes evidence and control portable rather than terminal-shaped | **Maturing** | **Essential direction; finish parity** |
| Budgets and context | Bound delegated work and reveal consumption | Wall-time, turn and API-spend caps plus spend/context records drive status and stopping | Budget is enforced by the outer harness | **Stable / bounded**: subscription quotas and context visibility vary by provider | **Preserve and extend** |
| Evidence ledgers and protocol | Make every important decision reconstructable | Append-only run, trace, spend, flight and narrative records feed a shared projection | Evidence is a first-class artifact, not a log side effect | **Stable**: Keel centralized the wire vocabulary, persistence policy, readers, writers and checked schemas | **Essential; preserve compatibility and route new formats through the protocol** |
| `RunView` projection | Give every surface one interpretation of run truth | Shared projection turns persisted records into status, show, reports and orchestration views | Prevents UI-specific truth and status drift | **Stable / maturing** | **Essential; make it the sole read model** |
| Independent gate, tamper evidence and verdict | Separate "agent finished" from "work accepted" | The gate runs as a separate subprocess; a successful exit and validated marker are required; verdict explains the result | This is the clearest differentiator from inner harnesses | **Maturing with a trust-claim gap** described below | **Highest-priority preserve and harden** |
| Snapshots, undo and rewind | Recover from autonomous mistakes without throwing away the whole run | File snapshots and diffs support undo; flight events provide checkpoint-like rewind | Recovery belongs to the outer control plane | **Stable / bounded**: file rollback is real; provider conversation rollback is not | **Preserve; state the boundary clearly** |
| Finish, promotion and library | Make acceptance and reuse explicit | Promotion revalidates the gate, stages changes atomically, and retains a searchable artifact/library record | The user accepts evidence-backed work, not an opaque session | **Hardened** | **Essential; preserve** |
| Attach / Helm | Observe long-running work from the operator's seat | A file-backed TUI reads the same durable state and supports bounded control actions | The UI can disappear without taking the run with it | **Stable / maturing** | **Essential operator surface; preserve and improve responsiveness** |
| Show, status, history and static report | Turn durable evidence into an inspectable artifact | Deterministic summaries and HTML reports render the shared projection | Strong local-first auditability without a service | **Stable / bounded** | **Preserve deterministic core; treat richer rendering as optional** |
| Docs and narration | Explain what happened at human scale | Deterministic docs plus optional provider-generated narrative summarize progress and artifacts | Evidence can be consumed without reading raw event streams | **Maturing / experimental**: several overlapping narrative paths | **Preserve one deterministic path; consolidate and validate model-generated variants** |
| Import | Bring work from other agent harnesses into the same evidence model | Parsers ingest supported transcript/session formats into local run artifacts | A partial cross-tool memory bridge | **Stable / bounded**: deliberately one-way, not shared live state | **Preserve; do not claim the broader need is solved** |
| Chains | Express sequential verified work | Each step is a normal run; outputs/status feed the next bounded step | Reuses the verified atom rather than inventing a workflow engine | **Stable** | **Strategic; preserve and consider as queue substrate** |
| Plans, fork/merge and review | Coordinate dependent work and reconcile branches | A saved DAG launches child runs, reviews results and merges accepted work | Every child remains independently gated | **Stable / maturing** | **Strategic; preserve, improve merge/recovery ergonomics** |
| Campaigns and reshape | Lift orchestration one level for broad goals | Bounded depth-two, currently serial sub-orchestrations can be reshaped under budget | Same evidence and gate model across nested work | **Experimental** despite substantive implementation | **Product decision**: validate real use before adding depth or concurrency |
| Seams | Let policy hooks compose without taking over the kernel | Four fixed subprocess seams receive versioned input and produce bounded output; conformance tooling checks them | Extensibility at explicit control points | **Stable / bounded** | **Preserve the fixed model; resist universal hook proliferation** |
| App-server steering | Control a live inner harness through a richer protocol | Codex app-server integration and a provider-neutral inbox support bounded steering/approvals | Moves beyond stdout scraping | **Experimental**, opt-in and Codex-led | **Preserve the neutral pattern; validate before broad surface investment** |
| Learning and self-improve | Mine prior runs and propose changes to DeadReckon itself | Indexing and learning commands derive observations/proposals from artifacts | A self-hosted improvement loop | **Experimental**; weak connection to the original highest-priority needs | **Strong product-decision/deprecation candidate** unless usage and proposal quality justify it |
| Notifications and sleep inhibition | Support unattended operation | Optional notifications and platform sleep handling wrap long runs | Useful operational polish | **Stable / peripheral** | **Keep while cheap; not strategic** |
| Doctor, setup, update and release trust | Make the binary installable and diagnosable | Environment checks, provider setup, update flow, signing/attestation and packaging support operations | Necessary for trusting a local supervisor binary | **Stable / maturing** | **Maintain as infrastructure, not as product differentiation** |

## The gate is essential, but its strongest claim is not yet proven

The operational completion invariant is real: the runtime invokes the gate, requires it to exit successfully, then validates the resulting marker before promotion. A pre-existing forged marker alone does not turn a failing gate into a passing one.

The stronger public claim—that an untrusted local agent *literally cannot forge* the receipt under every backend—is ahead of the implementation:

- The nonce is stored as a normal file in the run root (`deadreckon-core/src/state.rs`).
- The receipt signature uses Rust's `DefaultHasher`, not an HMAC or comparable secret-key construction (`deadreckon-core/src/gate.rs`).
- The macOS Seatbelt profile begins permissively and CLI/direct-agent invocations do not currently pass the run-root gate files as read-denied paths (`deadreckon-sandbox/src/commands.rs`, `deadreckon-providers/src/cli_common.rs`, `deadreckon-runtime/src/turn_loop.rs`).
- The `none` backend is explicitly available and provides no containment.
- Seam subprocesses *do* deny access to the gate and proof paths, showing the desired capability boundary, but that protection is not applied uniformly to all agent/tool execution paths (`deadreckon-runtime/src/seam.rs`).

Bubblewrap and Docker may provide stronger filesystem isolation by construction, but the property is not uniform across the supported backends. The accurate present description is therefore: **an enforced separate-process completion check with a tamper-evident receipt**, not yet a proven hostile-agent cryptographic boundary.

This does not make the gate cruft; it makes it the highest-priority hardening item. The already-drafted direction—HMAC-grade signatures, explicit nonce isolation, causal linkage and sandbox verification—is the correct closure.

## Original unmet needs: current outcome

The original research ranked 25 needs. The table distinguishes implemented primitives from the actual operator outcome.

| # | Unmet need | Current outcome | Assessment |
|---:|---|---|---|
| 1 | Live context and spend visibility | Spend records, caps, context meter and status exist; subscription/model telemetry remains uneven | **Partly met** |
| 2 | Multi-agent worktree coordination | Plans, chains and campaigns coordinate isolated runs; explicit fork-from-live-run, durable queue and resource broker do not exist | **Partly met** |
| 3 | Undo for agent changes | Snapshots, diff, undo and file rewind exist | **Strongly met**, within file-state scope |
| 4 | Provenance for generated code | Events, traces, artifacts and lineage are persisted | **Strongly met** |
| 5 | Searchable team memory | Local library, docs and import are searchable; shared team memory and automatic carryover are absent | **Partly met** |
| 6 | Cross-tool state | Multiple tools can be imported and providers share a control vocabulary; state is not bidirectionally live across tools | **Partly met** |
| 7 | Serious operator UI | Helm and reports provide dense terminal/static inspection; no live web/API or desktop control plane | **Partly met** |
| 8 | Observability, evals and root-cause analysis | Flight, traces, verdict and reports exist; automated RCA and cross-run evals do not | **Partly met** |
| 9 | Sandboxed execution | Multiple real backends and isolated codebases exist; automatic `none` fallback and backend differences weaken the default | **Mostly met; policy gap** |
| 10 | Billing guardrails | API spend, turn and wall caps exist; subscription quota semantics and universal usage accounting do not | **Partly met** |
| 11 | Permission controls | Sandbox policies, tool handling and app-server approvals exist | **Mostly met; parity and gate-secret caveat** |
| 12 | MCP client/server interoperability | No general MCP server or client surface | **Absent** |
| 13 | Team onboarding | Setup, doctor, provider discovery and contract inference exist | **Implemented; outcome unvalidated** |
| 14 | Structural verification of completion | Definition of done, independent gates, marker validation and promotion checks exist | **Core outcome met operationally; cryptographic claim needs hardening** |
| 15 | Discoverable hooks and gates | `def-done`, doctor, four seams and conformance tooling exist | **Strongly met** |
| 16 | Provider routing | Registry, capabilities, descriptors and routing cover several providers | **Mostly met; provider parity remains** |
| 17 | Handoff and continuity | Attach, status, extend and docs help a human resume; no first-class handoff artifact or memory carryover | **Partly met** |
| 18 | Port and environment isolation | Worktrees and process sandboxes isolate files/processes; there is no port/env lease broker | **Partly met** |
| 19 | Governance receipts | Gate receipts, evidence and promotion records exist; rules/skill receipts and cryptographic closure remain | **Mostly met** |
| 20 | Paid-review continuity | Review runs exist, but no specific paid-review workflow or continuity layer | **Mostly absent / not prioritized** |
| 21 | Local-first operation | Durable file-backed state and static artifacts are foundational | **Strongly met** |
| 22 | Meeting-to-code traceability | No dedicated workflow | **Absent / intentionally out of scope so far** |
| 23 | Prompt and team standards | Skills, contracts and seams provide primitives; rules-as-gate is drafted but not implemented | **Partly met** |
| 24 | Efficiency evaluation | Per-run spend/time/evidence exist; cross-run efficiency analysis and RCA do not | **Partly met** |
| 25 | Agent inventory and run queue | Runs can be listed and inspected; no durable scheduler/queue | **Partly met** |

## What is essential

These capabilities form the product's defensible spine and should be preserved, improved and extended:

1. **The verified run kernel**: durable state, locks, explicit lifecycle and deliberate promotion.
2. **Executable completion contracts**: definition of done, independent gate and explainable verdict.
3. **Isolation and recovery**: codebase modes, process sandboxing, snapshots, undo and retained failed evidence.
4. **Provider-neutral outer control**: one contract across direct APIs, CLI harnesses and structured app-server protocols.
5. **Evidence as a protocol**: append-only records, provenance, spend, flight data and a single `RunView` projection.
6. **Operator control without UI ownership**: attach/status/finish surfaces that read durable truth rather than holding it in memory.
7. **Composition from verified runs**: chains and dependency plans whose children use the same gate and promotion semantics.
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

- **Capstan process supervision**: process-group-safe cancellation and bounded output capture are specified but not yet complete.
- **Drydock determinism**: retry/timeout tests and deterministic fault injection are specified but not yet complete.
- **Provider parity**: structured events, context accounting, steering and approvals differ across adapters.
- **Sandbox default policy**: decide whether unavailable native isolation must fail closed for production modes rather than warn and use `none`.

### Product bets needing validation

- **Campaign depth**: useful implementation, but no evidence yet that depth-two sub-orchestration deserves more complexity or concurrency.
- **Generated/live narration**: human-readable summaries matter; multiple model-backed narration routes may not.
- **Codex app-server steering**: promising protocol integration whose provider-neutral inbox abstraction matters more than a Codex-specific surface.
- **Learning/self-improvement**: substantial code without a clear connection to the top operator needs. Require evidence of repeated high-quality proposals or deprecate it.
- **Flight rewind**: valuable as file recovery; do not imply semantic restoration of provider conversation unless that becomes a real protocol feature.

### Important unmet extensions

- Explicit gated `fork <run-id> --prompt ...` from a live or completed run.
- Durable run queue plus port/environment/resource leasing.
- Rules-as-gate and receipts showing which standards were applied.
- MCP server/client access to start, status, `RunView`, evidence and verdict.
- First-class handoff export and selective memory carryover between runs/tools.
- Cross-run RCA and efficiency comparison built from protocol records.
- A general provider-neutral approval/pause seam.
- Live read-only API/web projection, if terminal and static reports prove insufficient.

## Recommended sequence

1. **Correct and harden the trust boundary.** Align public language with current guarantees, implement cryptographic receipts, isolate nonce/proof paths for every backend, test hostile-agent access and make unsafe fallback explicit.
2. **Protect the protocol spine.** Keep durable readers and writers on the Keel vocabulary, enforce schema compatibility and make `RunView` the sole application projection.
3. **Close operational reliability.** Complete Capstan and Drydock before expanding long-running orchestration.
4. **Turn team policy into an acceptance input.** Deliver rules-as-gate on top of the existing done contract and receipt model.
5. **Add the missing coordination primitive.** Build explicit gated run forking, then use chains/plans as the basis for a small durable queue and resource broker.
6. **Expose, do not duplicate, the control plane.** Add MCP around existing lifecycle and `RunView` rather than creating another state model.
7. **Close continuity and learning at the evidence layer.** Add handoff/memory and cross-run RCA from protocol artifacts.
8. **Prune before adding more orchestration depth.** Remove compatibility no-ops and consolidate narration; require usage evidence for campaign expansion and self-improvement.

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
- adds orchestration depth before single-run supervision is fully trustworthy;
- is polished and tested but cannot be tied to an observed operator need.

## Evidence base and limits

This map was derived from the current CLI and crate graph, implementation paths, tests, changelog, goal/rider history, [`PRODUCT.md`](../PRODUCT.md), [`README.md`](../README.md), [`CONCEPTS.md`](CONCEPTS.md), [`AUDIT-2026-05-11.md`](AUDIT-2026-05-11.md), [`AS-BUILT-ARCHITECTURE.md`](AS-BUILT-ARCHITECTURE.md), and the July 2026 working unmet-needs reassessment. It also reconciles those artifacts with the original 2026-05-10 unmet-needs report in the adjacent Stoa research corpus.

The map is intentionally stricter than milestone labels such as “closed” or “stable”: those labels demonstrate implementation progress, while this document asks whether the operator outcome is actually met. Market maturity, frequency of use and willingness to pay cannot be inferred from this repository and remain validation questions.
