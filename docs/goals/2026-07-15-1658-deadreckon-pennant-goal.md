GOAL: Hoist a pennant per provider — declare each CLI agent's wire contract as descriptor data, not bespoke Rust. Semaphore gives codex and claude hand-written event mirrors; the other four CLI routes stay blind — and two are embarrassingly close: `cli:pi` already passes `--mode json --print` and `cli:copilot` already passes `--output-format json`, yet the generic driver dumps that JSON as raw response content with `usage: 0/0` — worse than plain text, narrators read blobs. Every generic provider is already pure TOML (`descriptors/*.toml`), so the contract belongs there too. This slice adds an optional `[contract]` descriptor section — stream args, JSON paths for conversation id / usage / answer / error, resume template, probe expectation — teaches the generic driver to honor it through Semaphore's shared machinery, and onboards the fleet: pi and copilot first (their JSON already flows), gemini and opencode if the installed binaries offer a structured mode when probed, everything grounded in fixtures recorded from real binaries. A new agent CLI's contract becomes a TOML edit plus fixtures — no code. Land this slice named Pennant.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-07-15-1658-deadreckon-pennant-rider.md` — `[contract]` schema, JSON-path dialect, per-provider onboarding rules, eleven phases, depth tests.
- `/Users/gdc/deadreckon/docs/goals/2026-07-11-1119-deadreckon-semaphore-rider.md` — prerequisite; Pennant constructs the `ProviderContract` Semaphore's machinery consumes, from TOML instead of code.
- `crates/deadreckon-providers/src/{cli_generic.rs,registry/mod.rs}` and `crates/deadreckon-providers/descriptors/{cli-pi,cli-copilot,cli-gemini,cli-opencode}.toml` — the exec templates and ingest schemas already there.
- The installed `pi`, `copilot`, `gemini`, `opencode` binaries (`--help` probes; fixture recording). Prior riders hold; Pennant takes AS-BUILT §55.

**Posture.** Stable track. Depends on Semaphore (machinery + session file + doctrine land there). `[contract]` is optional and additive: a descriptor without one behaves exactly as today; a malformed one is a load warning, never a run failure. Fields are declarative — args, JSON pointers, resume template — no expressions. Fixtures must be recorded from real binaries; a binary with no structured mode gets NO `[contract]` and a documented gap, not a guessed one. Tokens land where exposed; dollars stay subscription/$0. Resume only where the binary supports it. No `PipelineState` schema changes. No `git push`. Edits inside `/Users/gdc/deadreckon`. Decisions → V1-CANDIDATES.

**The fleet, onboarded honestly.**

- `cli:pi` — JSONL/JSON print mode already requested; contract declares id/usage/answer paths; resume per what the binary offers.
- `cli:copilot` — JSON already requested; same treatment.
- `cli:gemini`, `cli:opencode` — probe the installed binaries for a structured output mode; onboard what exists, document what doesn't.
- Response content for contract-bearing providers becomes the extracted answer — the raw-JSON-as-content wart dies.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `make verify` green → conventional-commit → CHANGELOG. P11 adds AS-BUILT §55.

**Verification.**

- Every rider depth test present and passing; `make verify` green each commit (scripted fake binaries per onboarded provider; no live CLIs in CI).
- A descriptor with `[contract]` yields real usage + extracted answer + (where declared) resume, via fixtures; a descriptor without one is byte-identical to today's behavior.
- Removing a contract field degrades that capability with a caveat — never a failed turn.

**Stop when** verification passes, AS-BUILT §55 + V1-CANDIDATES + a `Pennant (stable)` CHANGELOG section are updated, committed locally.
