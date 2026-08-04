<p align="center">
  <img src="docs/assets/deadreckon-wordmark.png" alt="deadreckon logo" width="640">
</p>

<p align="center"><strong>Run your coding agent unattended, and trust the result.</strong></p>

<p align="center">
DeadReckon is a harness around the agent CLI you already use.<br>
A guided <code>start</code> creates one durable Job and detached supervisor.<br>
Guided Single, Graph and Campaign Jobs require independent two-key verification.
</p>

---

## Get started

Install the latest release (macOS / Linux):

```bash
curl -fsSL https://deadreckon.sh/install.sh | sh
```

This resolves the newest release (stable preferred, release candidates included), verifies it against `SHA256SUMS`, and installs `deadreckon`. Pin any release tag with `DEADRECKON_TAG=<tag>`.

Then the whole tool is five commands:

| Command | What it does |
|---|---|
| `deadreckon start "build the app"` | Create a durable job, return its ID, and supervise the selected shape in isolation. |
| `deadreckon attach latest` | Watch it work live. `Ctrl-D` leaves it running. |
| `deadreckon status` | What happened, and the one thing to do next. |
| `deadreckon list` | Find recent runs and plans. |
| `deadreckon finish latest` | Apply it to your branch, or export it. |

Or build from source:

```bash
cargo build --release --workspace --locked
# coherent bundle at ./target/release/{deadreckon,dr-gate,dr-capture}
```

For a higher-level read while it runs, `deadreckon attach latest --view narrative` shows cited prose plus an evidence-backed visual map.

<p align="center">
  <img src="docs/assets/providers/claude-logo.png" alt="Claude Code" height="34">&nbsp;&nbsp;&nbsp;
  <img src="docs/assets/providers/openai-logo.png" alt="Codex" height="34">&nbsp;&nbsp;&nbsp;
  <img src="docs/assets/providers/gemini-logo.png" alt="Gemini CLI" height="34">&nbsp;&nbsp;&nbsp;
  <img src="docs/assets/providers/github-logo.png" alt="GitHub Copilot CLI" height="34">&nbsp;&nbsp;&nbsp;
  <img src="docs/assets/providers/opencode-logo.png" alt="OpenCode" height="34">&nbsp;&nbsp;&nbsp;
  <img src="docs/assets/providers/pi-logo.svg" alt="Pi" height="34">
</p>

<p align="center"><sub>Supervises the agent CLI you already use: Claude Code · Codex · Gemini · Copilot · OpenCode · Pi · or any Anthropic / OpenAI-compatible API.</sub></p>

Everything else (budgets, undo, multi-step chains, provider routing) is optional power you reach for later. No API keys? `deadreckon run "hello" --smoke --sandbox none` exercises the whole harness against a faked provider.

> [!TIP]
> `start`, ordinary direct `run` and `orchestrate`, new chains, stored-plan
> `fork`, direct campaigns, and public or guided follow-ups now enter the same
> durable Job scheduler. Each shape verifies its same-ID parent result with a
> native gate and a fresh read-only semantic judge before it validates a
> receipt and promotes the result. A follow-up freezes the parent state,
> promoted artifact, and verified receipt before queueing its isolated Single
> Job. Preview and explicit uncontained/in-place execution remain foreground,
> untrusted escape hatches. Public historical `chain run|resume` refuses
> before state mutation or execution. Public `chain extend` and `chain redo
> --extend` refuse before mutation and show the updated schedule to launch as
> a durable chain. Unsupported policy-rich chain creation also refuses instead
> of silently falling back to the old process-owned conductor. That conductor
> and its mutation paths remain available only in the characterization binary
> for tests.

## Why it's different

- **Two independent keys to “done.”** A guided Job is verified only when its
  deterministic checks pass inside a real sandbox and a fresh read-only
  semantic judge says the result meets the approved goal. Either key can
  refuse completion; the semantic judge cannot overrule a failed check.
- **Survives the launching terminal.** `start` freezes the goal, contract,
  policy, source digest, and launch plan before detaching a supervised process.
  Install and start the per-user supervisor service for restart-at-login
  posture; without it, durability is process-level rather than machine-level.
  `deadreckon doctor` shows which DeadReckon binaries, versions, install
  channels, receipt, and supervisor checkpoint are in play. `doctor --repair`
  can realign the running binary's metadata and a DeadReckon-managed service;
  it will not overwrite another package manager's copy.
- **Evidence, not a transcript.** A verified Job leaves an HMAC-SHA-256 receipt
  binding its authority, deterministic marker, semantic judgment, source and
  result digests, revisions, and confinement facts.

If `auto` cannot resolve to a real sandbox, or the semantic judge is unavailable
or uncertain, a strict Job becomes `NEEDS_REVIEW`; it is not silently accepted.
The repository has hermetic tests for these invariants. The cross-provider
20–30-task live dogfood matrix and live machine-restart acceptance are still
operator work, not completed claims.

→ The full story (the loop, the mental model, and how it compares) is in **[Concepts & How It Works](docs/CONCEPTS.md)**.

## Documentation

| Doc | What's inside |
|---|---|
| **[Concepts & How It Works](docs/CONCEPTS.md)** | The loop, the watchdog, the mental model, supported agents, how it compares |
| **[HOWTO](HOWTO.md)** | Practical usage: setup, providers, sandboxes, configuration, every command |
| **[DESIGN](DESIGN.md)** | Product and architecture intent |
| **[As-Built Architecture](docs/AS-BUILT-ARCHITECTURE.md)** | Detailed implementation reference |
| **[Watchkeeper acceptance](docs/WATCHKEEPER-OPERATOR-ACCEPTANCE.md)** | Manual tests for detach, service recovery, receipts, tamper refusal, and current limits |
| **[Resume Semantics](docs/RESUME-SEMANTICS.md)** · **[Multi-Run](docs/MULTI-RUN.md)** | Crash/resume behavior and concurrency rules |
| **[V1 Candidates](docs/V1-CANDIDATES.md)** | Deferred features and roadmap |

DeadReckon is maintained as production-release software. New execution through
the ordinary direct verbs and guided `start` shares one parent Job lifecycle
and a parent two-key receipt before promotion. Preview and explicit
in-place/uncontained execution are foreground and untrusted. Stored historical
chains remain inspectable, but their public execution and mutation commands
refuse and direct the operator toward a new durable chain.
