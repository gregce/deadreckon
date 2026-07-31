# deadreckon: Concepts & How It Works

The narrative companion to the [README](../README.md). If the README tells you
*what to type*, this explains *what deadreckon is doing and why you can trust
it*. For step-by-step usage, see [HOWTO](../HOWTO.md).

## How it works

1. **You write "done" in plain English.** deadreckon compiles it into executable checks: tests that must pass, files that must exist, scripts that must succeed.
2. **The agent runs in an isolated workspace.** Git sources default to a
   separate worktree. Non-Git sources default to a copy. DeadReckon requests a
   process sandbox, and a strict receipt requires a real backend to resolve.
   Explicit in-place compatibility mode is the workspace-isolation exception.
3. **Every turn is saved** (state, spend, traces, file provenance, snapshots) so you can attach, kill, resume, undo, or audit any moment.
4. **A separate watchdog process (`dr-gate`) runs the approved checks.** For a
   strict Job, the signing key is outside the agent workspace and protected
   paths are denied across the provider sandbox.
5. **A fresh read-only semantic judge checks the meaning.** It sees the goal,
   frozen contract, diff, deterministic evidence, and authority digests, but
   has no worker session or write posture. It returns `achieved`, `revise`, or
   `uncertain`.
6. **The trusted supervisor seals the final parent receipt.** Only a contained
   deterministic pass plus semantic `achieved` produces the HMAC-SHA-256
   two-key receipt required to promote a durable Job. A Single Job can use
   `revise` for another bounded turn. A Graph or Campaign Job can use `revise`
   for a bounded parent-only repair attempt without rerunning successful
   leaves.

The loop is the product. The agent CLI does the coding. deadreckon owns the
approved inputs, execution boundary, independent checks, and final receipt.

## Why this matters

**The agent does the coding. deadreckon decides when it's done.**

- **A contained Job cannot self-approve.** The worker cannot read the protected
  key or write the frozen authority/proof paths. The final receipt also needs
  an independent semantic judgment. If no real sandbox is available, the Job
  is uncontained and cannot be verified.
- **Walk away from the terminal.** Every turn and Job lifecycle fact is saved
  to disk. A detached Job supervisor keeps running after the launching
  shell exits. Install the per-user service for restart-at-login posture.
  Recovery may adopt or retry bounded work; it never treats process exit alone
  as proof of completion.
- **You get evidence, not a transcript.** Each accepted run promotes to a reviewable artifact: what changed, why, which prompt touched which file, what it spent. Auditable on disk, not scrolled back in a chat window.
- **Bring the agent you already trust.** Claude Code, Codex, Gemini, Copilot, OpenCode, Pi, or a raw API key: deadreckon supervises any of them. It owns the boundary, not the intelligence.

## Bring your own agent CLI

deadreckon owns the run boundary; the CLI does the coding. Route any turn through the agent you already trust:

| | Agent CLI (subscription) | Route |
|---|---|---|
| <img src="assets/providers/claude-logo.png" alt="" width="22"> | **Claude Code** | `cli:claude-code` |
| <img src="assets/providers/openai-logo.png" alt="" width="22"> | **Codex** | `cli:codex` |
| <img src="assets/providers/gemini-logo.png" alt="" width="22"> | **Gemini CLI** | `cli:gemini` |
| <img src="assets/providers/github-logo.png" alt="" width="22"> | **GitHub Copilot CLI** | `cli:copilot` |
| <img src="assets/providers/opencode-logo.png" alt="" width="22"> | **OpenCode** | `cli:opencode` |
| <img src="assets/providers/pi-logo.svg" alt="" width="22"> | **Pi** | `cli:pi` |

Prefer your own keys? Route directly to **Anthropic** (`anthropic`), **OpenAI** (`openai`), or any **OpenAI-compatible** endpoint (`openai-compatible`, e.g. OpenRouter, llama.cpp, local models). No keys at all? `--smoke` runs the whole harness against a faked provider.

Already have history in another tool? `deadreckon import claude-code | codex | cursor` ingests it read-only as a run you can inspect.

## The two things that make it different

### Write "done" in English, verified independently

Tell deadreckon what success looks like in plain language. It compiles your sentence into executable checks that an independent watchdog runs:

```bash
deadreckon def-done "build, load in a browser, and show no console errors"
deadreckon def-done add "users can save drawings"
deadreckon def-done check
deadreckon run "finish the app"
```

**Why the watchdog matters.** For a contained Job, `dr-gate` receives protected
signing material that is not visible to the worker and stamps the deterministic
check results. The runtime then asks a fresh read-only model to assess the
goal's meaning. The check marker is necessary but no longer sufficient: the
supervisor issues a verified receipt only when both decisions agree.

This stronger completion contract belongs to durable Jobs created through
guided `deadreckon start` and the ordinary direct execution verbs. Review and
full-plan work merges at the end, then becomes a same-ID Graph parent result.
Campaign completion also revalidates the worst-of leaf roll-up. Both paths
then run the native parent gate, ask a fresh read-only semantic judge, validate
the parent receipt and promote. Preview and explicit uncontained/in-place runs
remain foreground, untrusted escape hatches; their artifacts must not be
described as two-key Job receipts. Public historical `chain run|resume`
refuses before state mutation or execution. Public `chain extend` and `chain
redo --extend` refuse before mutation and offer the resulting schedule as a
new durable chain. Unsupported policy-rich chain creation also refuses rather
than silently using the process-owned conductor. Only the characterization
binary retains that legacy execution and mutation behavior for tests. Public
`deadreckon extend` and guided follow-up instead queue a parent-bound Single Job that
freezes and revalidates the completed parent's state, promoted-artifact tree,
and verified receipt when one exists.

If no acceptance file is configured, the default is "the working directory exists and `cargo test` passes" (when `Cargo.toml` is present). Supported check kinds are `cargo_test`, `file_exists`, `content_match`, `build_success`, and `shell`. Full reference, packs, and the compiled YAML format: [HOWTO § Done Criteria](../HOWTO.md#done-criteria).

### Get An Auditable Artifact, Not A Transcript

deadreckon records which model/tool call touched which files, then promotes each accepted run to a review packet:

- `RUN-NARRATIVE.md` explains what happened.
- `RUN-DECISIONS.md` records the decision ledger: design decisions, deviations, tradeoffs, open questions.
- `RUN-AS-BUILT.md` captures the subsystem shape after the run.
- `provenance.jsonl` links every changed file back to the turn, tool call, and model that produced it.
- `manifest.json` records what was built and from what.

Agentic CLIs usually leave you with a patch and a transcript. deadreckon turns the session into a local published artifact you can inspect, export, extend, or apply. Doc kinds and the TUI docs view: [HOWTO § Generated Docs](../HOWTO.md#generated-docs).

## What else it does

Each of these is a first-class capability; usage lives in [HOWTO](../HOWTO.md):

- **Source work is isolated by default.** Git sources use a separate
  `git worktree` on a `dr/...` branch. Non-Git sources use a copy. Your Git
  checkout changes only when you apply a result, unless you explicitly choose
  in-place compatibility mode. Fresh mode is also available.
- **Durable at two levels.** All runs persist state. Jobs created by `start`,
  ordinary `run`/`orchestrate`, new `chain`/`campaign`, and stored-plan `fork`
  also have an append-only lifecycle, a fenced renewable lease, process-group
  metadata, and a detached supervisor. The optional user service adds
  restart-at-login posture. Graphs and campaigns wrap their established
  conductor under that lease and verify the parent after merge. Preview and
  explicit in-place/uncontained execution remain foreground and untrusted.
- **Budgets and time limits.** `--max-spend 15` and `--max-wall-seconds 1800` cap a run, then walk away. High spend requires explicit confirmation.
- **Undo a single bad turn.** Snapshot-based rollback to any turn (`deadreckon undo <id> --turn 3`), recorded in the run trace, not just a `git reset`. The same verb unwinds a chain's last applied step.
- **Resume, kill, extend, or export any run.** Runs are lifecycle objects, not one terminal session.
- **Ordered multi-step work.** A new `deadreckon chain` compiles the steps into
  a linear Graph Job. Each child depends on the previous child. DeadReckon
  composes the result at the end, then verifies the same-ID parent once with
  both completion keys. Stored historical chains remain available for
  inspection, but public `chain run|resume` refuses before mutation or
  execution and points to a new durable chain. Public `chain extend` and
  `chain redo --extend` likewise refuse before mutation and show the updated
  durable schedule. The older stepwise gate, apply, and mutation model is
  reachable only through the characterization binary used by tests.
- **Retries instead of a pause.** A step that misses its done criteria is told exactly what failed and tried again — twice by default — before the plan decides whether the rest of the work continues. An unattended run should not stop and wait for someone who walked away.

## The mental model

```text
your repo
  |
  | deadreckon def-done "what 'finished' looks like, in English"
  | deadreckon start --mode run "what to build"
  v
isolated worktree or copy   ◄── your real checkout untouched
  |
  | provider route: cli:codex, cli:claude-code, cli:gemini, anthropic, openai, ...
  v
sandboxed turn loop         ◄── agent works here
  |
  | every turn: trace, spend, provenance, snapshot, docs
  | check fails? agent gets another turn
  v
dr-gate watchdog            ◄── separate process, uses protected HMAC key
  |                              contained agent cannot forge a valid marker
  | checks pass + HMAC marker valid?
  v
read-only semantic judge    ◄── goal + contract + diff + cited evidence
  |
  | achieved? seal two-key receipt
  | revise? another bounded worker or parent-repair turn
  | uncertain/unavailable? NEEDS_REVIEW
  v
promoted artifact           ◄── receipt, narrative, decisions, file lineage
  |
  | inspect, doc, apply, discard, extend, export, cleanup
  v
your branch or library
```

For multi-step work, guided or direct review/full-plan orchestration, new
chains, stored-plan `fork`, and campaigns put the established conductor under
one durable parent Job and lease. Child results are evidence, not parent
authority. The supervisor verifies the same-ID merged parent and promotes it
only after a valid two-key receipt. A semantic request to revise that parent
starts a fenced parent-only attempt, preserving the approved authority and
successful leaf results; repeated rounds remain bounded and linked in the
receipt evidence. A deterministic parent gate failure stops `FAILED`.
Stored historical chains are inspectable, but their public execution and
mutation paths refuse. Preview and explicitly uncontained/in-place execution
remains foreground and untrusted; the characterization binary alone retains
the old process-owned chain behavior for tests.

## Compared with agentic coding CLIs

deadreckon doesn't replace Claude Code, Codex, or the rest; it supervises them. Use it when the task is too long, risky, or expensive to "run the agent in my checkout and hope I can reconstruct what happened."

| Operational concern | Agentic CLI alone | deadreckon supervising the CLI |
|---|---|---|
| Workspace safety | Often runs where you start it | Separate worktree for Git sources and a copy for non-Git sources by default |
| Long-running attach | Tied to the current terminal | Durable state; guided `start` Jobs also detach their supervisor |
| Spend control | Varies by provider | Tracks totals; enforces spend and wall-clock caps |
| Undo | git-level or manual | Snapshots every turn; restores a specific turn |
| Done criteria | The agent may declare itself done | Strict Jobs require contained checks and semantic `achieved` |
| Output | A patch and a transcript | A promoted, auditable artifact with file provenance |

Use the agentic CLI for intelligence. Use deadreckon for isolation, supervision, evidence, recovery, and promotion.

## Why "deadreckon"?

Dead reckoning is navigation without perfect visibility: track every move so you know where you are now. Unattended agents are the same problem: long task, partial context, the terminal may be gone when you look back. The name is the contract: don't trust the final answer, navigate by evidence.
