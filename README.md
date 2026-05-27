<p align="center">
  <img src="docs/assets/deadreckon-wordmark.png" alt="deadreckon logo" width="640">
</p>

<p align="center"><strong>Run your coding agent unattended — and trust the result.</strong></p>

<p align="center">
A separate watchdog process, not the agent, decides when the work is actually done.<br>
You get a signed, auditable artifact instead of a chat transcript you have to take on faith.
</p>

<p align="center">
  <img src="docs/assets/providers/claude-logo.png" alt="Claude Code" height="34">&nbsp;&nbsp;&nbsp;
  <img src="docs/assets/providers/openai-logo.png" alt="Codex" height="34">&nbsp;&nbsp;&nbsp;
  <img src="docs/assets/providers/gemini-logo.png" alt="Gemini CLI" height="34">&nbsp;&nbsp;&nbsp;
  <img src="docs/assets/providers/github-logo.png" alt="GitHub Copilot CLI" height="34">&nbsp;&nbsp;&nbsp;
  <img src="docs/assets/providers/opencode-logo.png" alt="OpenCode" height="34">&nbsp;&nbsp;&nbsp;
  <img src="docs/assets/providers/pi-logo.svg" alt="Pi" height="34">
</p>

<p align="center"><sub>Supervises the agent CLI you already use — Claude Code · Codex · Gemini · Copilot · OpenCode · Pi · or any Anthropic / OpenAI-compatible API.</sub></p>

---

## Get started

```bash
cargo build --release          # binary at ./target/release/deadreckon
```

Then the whole tool is four commands:

| Command | What it does |
|---|---|
| `deadreckon start "build the app"` | Kick off a supervised run. Walk away. |
| `deadreckon attach latest` | Watch it work live. `Ctrl-D` leaves it running. |
| `deadreckon status` | What happened, and the one thing to do next. |
| `deadreckon finish latest` | Apply it to your branch, or export it. |

Everything else — budgets, undo, multi-step chains, provider routing — is optional power you reach for later. No API keys? `deadreckon run "hello" --smoke --sandbox none` exercises the whole harness against a faked provider.

> [!TIP]
> deadreckon is alpha software. The core lifecycle (isolated runs, signed gates, durable state, undo, docs, and apply) is implemented and tested, but expect rough edges and breaking changes.

## Why it's different

- **It can't fake the finish.** A separate watchdog (`dr-gate`) holds a secret the agent process can't read and signs the result with it. No valid signature, no "done" — the agent literally cannot mark its own work accepted.
- **Walk away for real.** Every turn is saved to disk. Close your laptop, lose the network, kill the model — attach from another terminal and resume from the last completed turn.
- **Evidence, not a transcript.** Each accepted run promotes to a reviewable artifact: what changed, why, which prompt touched which file, what it spent.

→ The full story — the loop, the mental model, and how it compares — is in **[Concepts & How It Works](docs/CONCEPTS.md)**.

## Documentation

| Doc | What's inside |
|---|---|
| **[Concepts & How It Works](docs/CONCEPTS.md)** | The loop, the watchdog, the mental model, supported agents, how it compares |
| **[HOWTO](HOWTO.md)** | Practical usage: setup, providers, sandboxes, configuration, every command |
| **[DESIGN](DESIGN.md)** | Product and architecture intent |
| **[As-Built Architecture](docs/AS-BUILT-ARCHITECTURE.md)** | Detailed implementation reference |
| **[Resume Semantics](docs/RESUME-SEMANTICS.md)** · **[Multi-Run](docs/MULTI-RUN.md)** | Crash/resume behavior and concurrency rules |
| **[V1 Candidates](docs/V1-CANDIDATES.md)** | Deferred features and roadmap |

deadreckon is alpha software. The core lifecycle is implemented and tested, alongside multi-agent orchestration (`plan` / `fork` / `merge`), autonomous chains, the provider flight recorder with `rewind`, and a local self-improvement loop (`learn` / `improve`).
