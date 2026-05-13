<p align="center">
  <img src="docs/assets/deadreckon-wordmark.png" alt="deadreckon logo" width="640">
</p>

# deadreckon

**A harness around the agent CLI you already use, so you can actually walk away.**

Codex, Claude Code, Aider, and the rest are good at writing code. They are not built to run unattended for hours and tell you, honestly, whether the work got done. deadreckon is.

You bring the agent CLI you already trust. You tell deadreckon what "done" looks like in plain English. It runs the work in an isolated sandbox, saves every turn, and uses a separate watchdog process to decide when the job is actually finished — a watchdog the agent cannot fool.

```bash
deadreckon done "users can sign up, log in, and save a drawing"
deadreckon run "build the app"
# walk away, attach later from any terminal
deadreckon attach latest
```

## How it works

1. **You write "done" in plain English.** deadreckon compiles it into executable checks — tests that must pass, files that must exist, scripts that must succeed.
2. **The agent runs in an isolated worktree**, inside a sandbox. Your real checkout is never touched.
3. **Every turn is saved** — state, spend, traces, file provenance, snapshots — so you can attach, kill, resume, undo, or audit any moment.
4. **A separate watchdog process (`dr-gate`) decides when the work is done.** It holds a secret the agent process cannot read, and signs the result with that secret. The agent cannot forge the signature, so it cannot mark its own work as accepted.
5. **If the checks fail, the loop keeps going.** The agent gets another turn. When the watchdog finally signs off for real, the run is atomically promoted to a reviewable artifact — narrative, decisions, file provenance, full audit trail.

The loop is the product. The agent CLI does the coding. deadreckon decides when "done" actually means done.

## Why this matters

- **You can leave a long run going and trust the result.** The agent can't lie its way out of the gate.
- **Use whichever agent CLI you prefer.** Claude Code, Codex, Aider, or direct Anthropic / OpenAI API — deadreckon supervises any of them.
- **You get an auditable artifact, not a chat transcript.** Narrative, decisions, file lineage, spend, traces — all on disk.
- **If anything dies — terminal, network, the model — the run survives.** Attach from a new terminal, resume from any turn, undo a bad step.

## Why "deadreckon"?

Dead reckoning is navigation without perfect visibility: start from a known
position, keep a continuous record of movement, and use that record to know
where you are now.

That is the problem with unattended coding agents. The model is probabilistic,
the task can run for a long time, context is partial, and the terminal may be
gone by the time you inspect the result. deadreckon keeps the course anyway:
every turn writes state, spend, traces, provenance, snapshots, docs, and
acceptance evidence.

The name is the contract: do not trust a final answer alone. Navigate agent work
by evidence.

## Why This Exists

Agentic coding CLIs are good at the inner loop: talk to a model, inspect files, run commands, edit code, and produce a patch. Amp, Rovo Dev, Cursor CLI, Codex, Claude Code, Aider, deepseek-tui, GitHub Copilot CLI, and similar tools are racing on model UX, tool UX, context handling, and terminal/editor ergonomics.

deadreckon focuses on the layer around that CLI: the unattended run lifecycle.

The painful part of unattended coding is different:

- Where exactly did the agent run?
- What did it spend?
- Which files did each turn touch?
- Can I attach after closing my laptop?
- Can I kill it without leaving orphaned processes?
- Can I resume after a crash?
- Can I undo turn 7 without rolling back the whole project?
- Can I prove the agent did not mark its own work as accepted?
- Can I apply the result only after I inspect it?

deadreckon is built around those questions. It treats an agentic CLI as the worker, then adds the control plane that makes unattended work reviewable, recoverable, auditable, and safe to apply.

## The Printing Press Contract

deadreckon borrows a simple idea from the CLI Printing Press pattern: split
judgment from enforcement.

The agentic CLI and Markdown skills own the judgment layer: what to inspect,
what to try, what to edit, and when to say the work is done. The Rust binary owns
the enforcement layer: state, locks, sandboxes, provider routing, cancellation,
snapshots, provenance, spend, traces, gates, promotion, and recovery.

That split matters. The agent can write code, but it cannot forge completion,
skip the audit trail, bypass run state, or silently publish its own work.

## The Unique Feature Set

### Your Checkout Is Never Touched (Isolated Worktrees By Default)

In a git repo, `deadreckon run` creates a separate `git worktree` on a `dr/...` branch under `~/.deadreckon/worktrees/`. Your checkout is left untouched until you explicitly run:

```bash
deadreckon apply <run-id>
```

Run ids accept unique prefixes, and most commands also accept `latest` for the
newest run in the current project.

You can also run in copy mode, fresh mode, or explicit in-place mode:

```bash
deadreckon run "goal" --from .
deadreckon run "goal" --fresh
deadreckon run "goal" --in-place --i-know-its-a-lot
```

### Use The Agent CLI You Already Trust

deadreckon routes turns through configurable providers. Today, the first-class local CLI adapters are:

- `cli:codex`
- `cli:claude-code`

It also supports direct model routes:

- `anthropic`
- `openai`
- OpenAI-compatible endpoints
- `--smoke` for keyless local verification

That means subscription users can route through supported local agentic CLIs, while API users can route through direct HTTP providers. The CLI does the coding; deadreckon owns the run boundary around it.

### Crash-Proof: Every Turn Is Saved To Disk

Each run writes a complete local record under `~/.deadreckon/runstate/`:

```text
state.json
history.json
events.jsonl
traces.jsonl
spend.jsonl
provenance.jsonl
snapshots/turn-<N>/
proofs/turn-acceptance.json
working/ or promoted library artifact
```

If the terminal dies, the run state is still there. If a provider call completes, the trace is there. If a tool edits a file, the provenance is there.

### Walk Away, Attach From Any Terminal

Start a run, walk away, then attach from another terminal:

```bash
deadreckon attach latest
```

The TUI shows compact run status, phase, goal, working directory, per-turn timer,
spend or context telemetry, provider/tool activity, generated files, process
status, and completion actions. Completed runs can toggle a styled Markdown
rendering of `RUN-NARRATIVE.md` directly in the main panel.

Detach without killing the run:

```text
Ctrl-D
```

### Set A Budget And A Time Limit, Then Walk Away

Every provider response appends a spend record and updates totals. API routes track token cost. Subscription CLI routes can be capped by wall-clock time.

```bash
deadreckon run "large refactor" --max-spend 15
deadreckon run "large refactor" --max-wall-seconds 1800
```

High spend requires explicit confirmation, so scripts do not accidentally launch expensive runs.

### Undo A Single Bad Turn Without Losing The Rest

deadreckon snapshots the working directory at turn boundaries:

```bash
deadreckon undo --run <run-id>
deadreckon undo --run <run-id> --turn 3
```

This is not just `git reset`. It works against the run's own snapshot trail and records the undo in the run trace.

### Write "Done" In English, Verified By A Watchdog

Tell deadreckon what success looks like in plain language. It compiles your sentence into executable checks that an independent watchdog runs:

```bash
deadreckon done "build, load in a browser, and show no console errors"
deadreckon done add "users can save drawings"
deadreckon done check
deadreckon run "finish the app"
```

`deadreckon run` and `deadreckon chain run` prompt interactively when a project has no acceptance file yet. Generated files live under `.deadreckon/acceptance.yaml`, `.deadreckon/acceptance.md`, and optional helper scripts in `.deadreckon/acceptance/`.

**Why the watchdog matters.** Completion requires a marker file written by the separate `dr-gate` binary. That binary holds a secret (a per-run nonce) that lives outside the agent's sandbox-readable paths. It hashes the secret together with the check results to sign the marker. deadreckon refuses any marker whose signature doesn't validate against that secret — so even if the agent fabricates a perfect-looking "I'm done" file, it gets rejected.

The agent runs the code. The watchdog decides whether the code passed. They are different processes with different filesystem reach. The agent cannot become the watchdog.

If no acceptance file is configured, the default is the working directory exists and `cargo test` passes (when `Cargo.toml` is present).

You can still edit the compiled YAML directly:

```yaml
name: notes check
checks:
  - kind: file_exists
    path: "{working_dir}/notes.md"
  - kind: content_match
    path: "{working_dir}/notes.md"
    pattern: "dead reckoning"
  - kind: build_success
    cwd: "{working_dir}"
```

Supported executable check kinds are `cargo_test`, `file_exists`, `content_match`, `build_success`, and `shell`. `content_match` treats `pattern` as a regex when valid, with substring fallback for simple text.

### Get An Auditable Artifact, Not A Transcript

deadreckon records which model/tool call touched which files, then writes self-documenting artifacts for the completed run:

```bash
deadreckon doc <run-id>
deadreckon doc <run-id> --kind as-built
deadreckon doc <run-id> --kind decisions
deadreckon doc <run-id> --kind delta
```

Each accepted run produces a review packet:

- `RUN-NARRATIVE.md` explains what happened.
- `RUN-AS-BUILT.md` captures the subsystem shape after the run.
- `RUN-DECISIONS.md` records meaningful decisions.
- `AS-BUILT-DELTA.md` proposes architecture-doc updates when the run is broad enough.
- `manifest.json` records the promoted artifact identity and provenance hash.

Agentic CLIs usually leave you with a patch and a transcript. deadreckon turns
the session into a local published artifact you can inspect, materialize, extend,
or apply.

### Resume, Kill, Extend, Or Export Any Run

Runs are lifecycle objects, not one terminal session:

```bash
deadreckon list
deadreckon status
deadreckon show latest
deadreckon kill latest
deadreckon resume latest
deadreckon resume latest --from-turn 2
deadreckon extend latest "add tests and polish the UI"
deadreckon finish latest
deadreckon export latest --dest ./finished-project
deadreckon cleanup --completed
```

Resume reconstructs history from durable traces and ignores incomplete trailing trace entries.

### Autonomous Chains For Multi-Step Work

Some tasks are too big for one goal. Chains let you break work into ordered steps that run end-to-end, with the same gate enforcement and the same lifecycle commands per step:

```bash
deadreckon chain plan "ship a working SaaS billing flow" --n 4
deadreckon chain run latest
deadreckon chain attach latest
deadreckon chain kill latest
```

Each chain step is a real run with its own signed acceptance gate. If a step fails its gate, the chain stops there — later steps don't start. Killing a chain cascades to whatever step is live. The attach TUI shows the whole chain timeline, not just the current turn, so you can see where you are in the plan without leaving the dashboard.

You can also extend a finished run into a follow-up step instead of planning the whole chain up front:

```bash
deadreckon chain extend latest "add billing webhooks and retry logic"
```

## The Mental Model

```text
your repo
  |
  | deadreckon done "what 'finished' looks like, in English"
  | deadreckon run  "what to build"
  v
isolated worktree or copy   ◄── your real checkout untouched
  |
  | provider route: cli:codex, cli:claude-code, anthropic, openai, ...
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

For multi-step work, `deadreckon chain` wraps this whole loop and runs N of them in order, stopping on the first gate failure.

deadreckon owns the boring but load-bearing parts: state, locks, sandboxes, provider routing, cancellation, snapshots, provenance, gates, and promotion.

The agent owns the coding work.

## The Trust Contract

With deadreckon, `completed` means more than "the agent stopped." It means:

- the run state was persisted
- the work happened in an isolated workspace
- spend and traces were written
- file provenance was recorded
- snapshots exist for rollback
- run docs were generated
- `dr-gate` signed acceptance
- the artifact was promoted atomically

That is the difference between an agent transcript and an auditable run.

## Quickstart

Build the release binary:

```bash
cargo build --release
```

Initialize local config:

```bash
./target/release/deadreckon init
```

Run a task:

```bash
./target/release/deadreckon run "make a realtime chess app"
```

Attach to the run:

```bash
./target/release/deadreckon attach <run-id>
```

Inspect the result:

```bash
./target/release/deadreckon show <run-id>
./target/release/deadreckon doc <run-id>
```

Apply a completed worktree run:

```bash
./target/release/deadreckon apply latest --autostash --cleanup
```

## Keyless Smoke Test

Use `--smoke` to prove the harness works without API keys or subscription-backed CLIs:

```bash
DEADRECKON_HOME=$PWD/.deadreckon-smoke \
  ./target/release/deadreckon run "tiny hello rust" \
  --smoke \
  --sandbox none \
  --max-spend 1
```

This still exercises the real turn loop, sandbox dispatch, snapshots, spend records, traces, provenance, docs, and acceptance gate. The only fake part is the provider response.

## Normal Coding Run

After `init`, the default path in a git repo is:

```bash
deadreckon run "make a full task productivity tracker in nodejs that allows me to manage my day"
deadreckon attach latest
deadreckon status
deadreckon doc latest
deadreckon apply latest --autostash --cleanup
```

`run` prints the run id and attach command immediately:

```text
started run <short-id> (<full-id>)
attach: deadreckon attach <short-id>
```

Before creating state or files, `run` prints a preview showing the mode, branch, base ref, and worktree path. Use `--preview` to print the preview and exit.

## Provider Configuration

Examples:

```bash
deadreckon init --provider cli:codex --sandbox auto --max-spend 10 --no-confirm
deadreckon init --provider cli:claude-code --sandbox auto --max-spend 10 --no-confirm
deadreckon init --provider anthropic --api-key "$ANTHROPIC_API_KEY" --sandbox auto --max-spend 10 --no-confirm
deadreckon init --provider openai --api-key "$OPENAI_API_KEY" --sandbox auto --max-spend 10 --no-confirm
```

Inspect or edit config:

```bash
deadreckon config get defaults.provider
deadreckon config provider
deadreckon config provider cli:codex
deadreckon config model
deadreckon config model gpt-5.1-codex --provider cli:codex
deadreckon config set defaults.max_spend 15
deadreckon config set defaults.sandbox auto
deadreckon config set providers.anthropic.api_key "$ANTHROPIC_API_KEY"
```

Before a run, `deadreckon run --preview "goal"` shows the provider route and
model that will be used. For one run, override both directly:

```bash
deadreckon run "goal" --provider cli:codex --model gpt-5.1-codex
deadreckon run "goal" --provider cli:claude-code --model sonnet
```

Runtime config defaults to:

```text
~/.deadreckon/config.toml
```

Set `DEADRECKON_HOME` for isolated local runs or tests.

## Sandbox Backends

Supported backends:

```text
auto
sandbox-exec
bwrap
docker
none
```

`auto` chooses `sandbox-exec` on macOS when available and `bwrap` on Linux when available. `docker` is opt-in. `none` is explicit and warns because it is unsafe for real unattended work.

Check the current machine:

```bash
deadreckon doctor
```

## Compared With Agentic Coding CLIs

deadreckon is not a replacement for agentic CLIs like Amp, Rovo Dev, Cursor CLI, Codex, Claude Code, Aider, deepseek-tui, or GitHub Copilot CLI. It is the supervisor you put around that class of tool when the task is too long, risky, or expensive for "run the agent in my checkout and hope I can reconstruct what happened."

| Operational concern | Agentic CLI alone | deadreckon supervising the CLI |
|---|---|---|
| Workspace safety | Often runs where you start it | Creates an isolated worktree by default and leaves your checkout untouched |
| Long-running attach | Usually tied to the current terminal session | Run state is durable; attach from another terminal later |
| Process control | Kill/resume behavior varies by tool | Tracks child PIDs, kills live work, resumes from durable state |
| Spend control | Token/cost visibility varies by provider | Writes `spend.jsonl`, tracks totals, enforces spend and wall-clock caps |
| Undo | Usually git-level or manual | Snapshots every turn and restores a specific turn with `deadreckon undo` |
| Provenance | Conversation history may not map cleanly to file changes | Records model/tool/file linkage in `provenance.jsonl` |
| Observability | Tool logs are tool-specific | Writes normalized `events.jsonl`, `traces.jsonl`, spend, docs, and run state |
| Done criteria | The agent may declare itself done | Requires a signed `dr-gate` marker before promotion |
| Applying work | Patch review/apply flow is tool-specific | `apply`, `discard`, `export`, `extend`, and `cleanup` are first-class lifecycle actions |
| Multi-run coordination | Usually left to the operator | Scope/task locks prevent conflicting same-task runs |

Use the agentic CLI for intelligence. Use deadreckon for isolation, supervision, evidence, recovery, and promotion.

## Command Surface

```text
init          create local config
config        inspect or edit config keys
run           start an unattended coding run
attach        open the live dashboard
status/next   show the current project's latest run and next action
list          show current-project runs by default; --all shows every scope
show          inspect state, lineage, spend, files
doc           print or export run documentation
finish        choose apply or export from the completed run mode
apply         apply a completed worktree run to your branch
keep          alias for apply
abandon       remove a worktree run and temporary branch
discard       alias for abandon
materialize   copy a completed artifact to a normal directory
export        alias for materialize
cleanup       clean abandoned, stale, or completed worktrees
prune         alias for cleanup
extend        continue from a completed run
follow-up     alias for extend
resume        continue an interrupted run
continue      alias for resume
kill          stop a live run and child processes
stop          alias for kill
undo          restore a previous turn snapshot
import        normalize histories from other coding tools
doctor        check config, providers, sandboxes, disk, runtime
```

## Verification

Full local verification:

```bash
make verify
```

Useful targeted checks:

```bash
make build
make smoke
make doctor
STRESS_SECONDS=30 make stress
```

The workspace test suite covers provider routing, mock OpenAI-compatible runs, CLI-provider wrappers, kill/resume behavior, signed gates, snapshots, provenance/trace linkage, worktree/copy/in-place modes, lifecycle actions, docs generation, and multi-run locking.

## Status

deadreckon is alpha software. The core lifecycle is implemented and tested, but the project is intentionally conservative about claims: local-first state, worktree isolation, provider routing, sandbox dispatch, durable traces/spend/provenance, signed gates, undo, docs, apply/abandon/materialize/extend, and smoke verification are the current product.

The V1 candidate list is deliberately short. The biggest known next feature is explicit sub-agent forking:

```text
deadreckon fork <run-id> --prompt "..."
```

## Documentation

- [HOWTO.md](HOWTO.md): practical usage guide
- [docs/DEVELOPMENT-README.md](docs/DEVELOPMENT-README.md): preserved developer-oriented README notes
- [DESIGN.md](DESIGN.md): product and architecture intent
- [docs/AS-BUILT-ARCHITECTURE.md](docs/AS-BUILT-ARCHITECTURE.md): detailed implementation reference
- [docs/RESUME-SEMANTICS.md](docs/RESUME-SEMANTICS.md): crash and resume semantics
- [docs/MULTI-RUN.md](docs/MULTI-RUN.md): lock ordering and concurrency rules
- [docs/GAP-ANALYSIS.md](docs/GAP-ANALYSIS.md): primary-flow audit
- [docs/V1-CANDIDATES.md](docs/V1-CANDIDATES.md): deferred features
