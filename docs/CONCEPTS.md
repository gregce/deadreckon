# deadreckon: Concepts & How It Works

The narrative companion to the [README](../README.md). If the README tells you
*what to type*, this explains *what deadreckon is doing and why you can trust
it*. For step-by-step usage, see [HOWTO](../HOWTO.md).

## How it works

1. **You write "done" in plain English.** deadreckon compiles it into executable checks: tests that must pass, files that must exist, scripts that must succeed.
2. **The agent runs in an isolated worktree**, inside a sandbox. Your real checkout is never touched.
3. **Every turn is saved** (state, spend, traces, file provenance, snapshots) so you can attach, kill, resume, undo, or audit any moment.
4. **A separate watchdog process (`dr-gate`) decides when the work is done.** It holds a secret the agent process cannot read, and signs the result with that secret. The agent cannot forge the signature, so it cannot mark its own work as accepted.
5. **If the checks fail, the loop keeps going.** The agent gets another turn. When the watchdog finally signs off for real, the run is atomically promoted to a reviewable artifact: narrative, decisions, file provenance, full audit trail.

The loop is the product. The agent CLI does the coding. deadreckon decides when "done" actually means done.

## Why this matters

**The agent does the coding. deadreckon decides when it's done.**

- **It can't fake the finish.** The watchdog (`dr-gate`) holds a secret the agent process can't read and signs the result with it. No valid signature, no "done" — the agent literally cannot forge its own acceptance.
- **Walk away for real.** Every turn is saved to disk: state, spend, file lineage, a full snapshot. Close your laptop, lose the network, kill the model — attach from another terminal and resume from the last completed turn. Nothing replays, nothing is lost.
- **You get evidence, not a transcript.** Each accepted run promotes to a reviewable artifact: what changed, why, which prompt touched which file, what it spent. Auditable on disk, not scrolled back in a chat window.
- **Bring the agent you already trust.** Claude Code, Codex, Gemini, Copilot, OpenCode, Pi, or a raw API key — deadreckon supervises any of them. It owns the boundary, not the intelligence.

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

Prefer your own keys? Route directly to **Anthropic** (`anthropic`), **OpenAI** (`openai`), or any **OpenAI-compatible** endpoint (`openai-compatible` — OpenRouter, llama.cpp, local models). No keys at all? `--smoke` runs the whole harness against a faked provider.

Already have history in another tool? `deadreckon import claude-code | codex | cursor` ingests it read-only as a run you can inspect.

## The two things that make it different

### Write "Done" In English, Verified By A Watchdog The Agent Can't Fool

Tell deadreckon what success looks like in plain language. It compiles your sentence into executable checks that an independent watchdog runs:

```bash
deadreckon def-done "build, load in a browser, and show no console errors"
deadreckon def-done add "users can save drawings"
deadreckon def-done check
deadreckon run "finish the app"
```

**Why the watchdog matters.** `dr-gate` holds a run-local secret the agent process cannot read, and stamps the result with it. Without a valid stamp, the run can't terminate — and the agent can't produce the stamp itself. If the checks fail, the run doesn't end; the agent gets another turn with a corrective hint.

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

Each of these is a first-class capability — usage lives in [HOWTO](../HOWTO.md):

- **Your checkout is never touched.** Runs default to an isolated `git worktree` on a `dr/...` branch; your real checkout changes only when you `deadreckon apply`. Copy, fresh, and explicit in-place modes are available too.
- **Crash-proof.** Every turn writes durable state. If the terminal dies, attach from another; if the run crashes, resume from the last completed turn.
- **Budgets and time limits.** `--max-spend 15` and `--max-wall-seconds 1800` cap a run, then walk away. High spend requires explicit confirmation.
- **Undo a single bad turn.** Snapshot-based rollback to any turn (`deadreckon undo --run <id> --turn 3`), recorded in the run trace — not just a `git reset`.
- **Resume, kill, extend, or export any run.** Runs are lifecycle objects, not one terminal session.
- **Autonomous chains for multi-step work.** Break a big goal into ordered steps, each with its own signed gate; the chain stops on the first gate failure.

## The mental model

```text
your repo
  |
  | deadreckon def-done "what 'finished' looks like, in English"
  | deadreckon run  "what to build"
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
dr-gate watchdog            ◄── separate process, holds hidden nonce
  |                              agent CANNOT produce a valid marker
  | checks pass + signature valid?
  v
promoted artifact           ◄── narrative, decisions, file lineage
  |
  | inspect, doc, apply, discard, extend, export, cleanup
  v
your branch or library
```

For multi-step work, `deadreckon chain` wraps this whole loop and runs N of them in order, stopping on the first gate failure. The agent owns the coding. deadreckon owns the boundary.

## Compared with agentic coding CLIs

deadreckon doesn't replace Claude Code, Codex, or the rest; it supervises them. Use it when the task is too long, risky, or expensive to "run the agent in my checkout and hope I can reconstruct what happened."

| Operational concern | Agentic CLI alone | deadreckon supervising the CLI |
|---|---|---|
| Workspace safety | Often runs where you start it | Isolated worktree by default; your checkout untouched |
| Long-running attach | Tied to the current terminal | Durable state; attach from another terminal later |
| Spend control | Varies by provider | Tracks totals; enforces spend and wall-clock caps |
| Undo | git-level or manual | Snapshots every turn; restores a specific turn |
| Done criteria | The agent may declare itself done | Requires a signed `dr-gate` marker before promotion |
| Output | A patch and a transcript | A promoted, auditable artifact with file provenance |

Use the agentic CLI for intelligence. Use deadreckon for isolation, supervision, evidence, recovery, and promotion.

## Why "deadreckon"?

Dead reckoning is navigation without perfect visibility: track every move so you know where you are now. Unattended agents are the same problem — long task, partial context, the terminal may be gone when you look back. The name is the contract: don't trust the final answer, navigate by evidence.
