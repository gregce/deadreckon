# deadreckon — Build Rider (V0)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-10-deadreckon-build-goal.md`.
Read it before writing code. Sections marked "do not violate" are hard constraints.

**All paths in this document are absolute. Generated code, config, docs, and
verification scripts must use absolute paths or paths anchored to a single
known root (`/Users/gdc/deadreckon/` for source, `/Users/gdc/.deadreckon/`
for runtime state). No bare relative paths in user-visible artifacts.**

## Product framing (decided — do not redesign)

- **What it is.** A Rust CLI agent harness whose default flow is **unattended long-running coding tasks**. The user types one goal, hits enter, and the harness drives the task across hours of model time — sandboxed, crash-resumable, spend-capped, with full provenance.
- **What it isn't.** Not an editor, not a chat app, not an MCP framework, not a CI runner, not a kitchen-sink agent IDE.
- **Target user.** Solo developers and ≤ 10-person teams currently paying $200+/month per seat across Claude Code, Codex CLI, Cursor, opencode, and still doing manual bookkeeping for spend, undo, and multi-agent coordination.
- **BYOK posture.** Multi-provider from day one, opencode-style. The user supplies keys; the harness never bills usage. V0 providers: Anthropic, OpenAI, plus an OpenAI-compatible shim (OpenRouter, llama.cpp, etc.).
- **Name.** **deadreckon.** Dead reckoning is navigation by integrating known motion from a fixed starting point — no GPS, no external check. The harness drives long-running agent work the same way: durable state at every turn, no live human in the loop, no live model attestation, reconstructable position after any crash.

## Architecture (decided — do not redesign)

Adopt these patterns from `/Users/gdc/Downloads/AS-BUILT-ARCHITECTURE.md`. Each
adoption cites the AS-BUILT section in a code comment near the implementation.

- **Two-layer split.** Rust binary owns state, locks, sandboxes, gates, atomic file ops, sub-agent management. Agent-facing skill layer (markdown skills loaded at runtime) owns judgment, prose, orchestration. The skill is invoked as a sub-process; the binary never reaches into skill internals. (AS-BUILT §3)
- **Phase machine with on-disk state.** `state.json` per run, status enum (`pending|planned|executing|completed|failed`), gap-numbered phase IDs. (AS-BUILT §4)
- **Run pointer + run scoping.** `current/<task>.json` points at the active run per task; `runs/<run-id>/` carries durable state. Scope hash from git root or env var lets parallel worktrees coexist. (AS-BUILT §4)
- **Locks with heartbeats + PID liveness.** Process-held locks survive crash detection via PID-zero kill probe; stale locks reclaimable after timeout. (AS-BUILT §6)
- **Atomic promotion.** Work happens in `working/`; promoted atomically to `library/` only after a gate marker is written. (AS-BUILT §7)
- **Anti-self-attestation gates.** Acceptance markers (test pass, sandbox green, spend within cap) can only be written by an external runner and are validated by the binary against the run identity. The agent cannot self-mark a gate. (AS-BUILT §8, §17)
- **Sub-agent forking with context isolation.** Research-heavy or diagnostic-heavy sub-tasks run in forked subprocess agents; they return structured summaries to the parent. (AS-BUILT §10)
- **Bounded fix loops with rollback.** Every code mutation is backed up, build-checked, regression-checked, and rolled back on failure within a retry budget. (AS-BUILT §9)

## Patterns to mine from Claude Code source (`/Users/gdc/claude-code-source-code/`)

The Claude Code source is a working reference for the user-facing primitives
this harness needs. Read these subtrees before implementing the relevant
crate; cite the file in a code comment when adopting a pattern.

- **Tool-use loop and turn structure** — `/Users/gdc/claude-code-source-code/src/assistant/`. How the assistant runs a turn, schedules tool calls, handles streaming, and unwinds on errors. Inform `deadreckon-core`'s run loop.
- **Tool implementations and permission model** — `/Users/gdc/claude-code-source-code/src/tools/`. Per-tool permission gates, parameter schemas, side-effect boundaries, the analogue of `--dangerously-skip-permissions`. Inform `deadreckon-core` tool dispatch, the platform-sandbox profile (filesystem/network/env scope), and the `--sandbox none` warning.
- **Task / coordinator orchestration** — `/Users/gdc/claude-code-source-code/src/tasks/`, `/Users/gdc/claude-code-source-code/src/coordinator/`. Multi-task supervision, sub-agent forking patterns. Inform `deadreckon`'s `run/attach/kill/resume`.
- **Upstream proxy / provider abstraction** — `/Users/gdc/claude-code-source-code/src/upstreamproxy/`. How Claude Code shapes requests, handles streaming, retries, and tool-result feedback. Inform `deadreckon-providers`.
- **TUI patterns** — `/Users/gdc/claude-code-source-code/src/ink/`. React-Ink reference for a streaming, attachable terminal UI; port idioms to `ratatui` rather than copying React.
- **Prompts and skill structure** — `/Users/gdc/claude-code-source-code/docs/prompts/`. Existing system prompt shapes, skill loading conventions. Inform `/Users/gdc/deadreckon/skills/default-coding/SKILL.md`.

**Rust adaptations (use these crates, no alternatives in V0):**

- `tokio` — async runtime.
- `serde` + `serde_json` — state serialization.
- `nix` (Unix) + `fs2` — advisory file locking + PID liveness via `kill(pid, 0)`.
- `tracing` + `tracing-subscriber` — structured logs / observability.
- `clap` derive — CLI parsing.
- `ratatui` + `crossterm` — TUI.
- `reqwest` — HTTP client for provider calls.
- `which` — locate `sandbox-exec` / `bwrap` / `docker` binaries at preflight time. Sandbox backends shell out via `tokio::process::Command`; no Docker SDK dep.
- Cargo workspace: separate crates for `deadreckon-core`, `deadreckon-providers`, `deadreckon-sandbox`, `deadreckon` (bin).

## Must-have unmet needs (V0 scope)

Each implementation slice maps to ≥ 1 of these. Comment in the source where
each is implemented (e.g., `// REPORT.md: Live Context & Spend Meter`).

1. **Live Context & Spend Meter** — `/Users/gdc/.deadreckon/runstate/<scope>/runs/<id>/spend.jsonl` (durable per-turn telemetry); surfaces in TUI.
2. **Multi-Agent Worktree Coordination** — scopes + lock primitives let N agents run in parallel without conflict.
3. **Infinite Undo for Agent Edits** — `snapshots/turn-<N>/` per run; `deadreckon undo` restores to any prior turn.
4. **Prompt-to-Code Provenance Audit Trail** — `provenance.jsonl` per run; every file change carries `{ prompt_id, model, tool_call_id, timestamp, session_id, files: [...] }`.
5. **Cross-Tool State Sharing (read-only import)** — `deadreckon import claude-code|codex|cursor` reads `/Users/gdc/.claude/projects/`, `/Users/gdc/.codex/sessions/`, `/Users/gdc/.cursor/chats/`. No live bridging in V0.
6. **Agent Observability** — `traces.jsonl` per run (LLM calls, tool calls, retries, latency); exportable.
7. **Disposable Sandboxes** — platform-native per run by default (`sandbox-exec` on macOS, `bubblewrap` on Linux); `--sandbox auto|sandbox-exec|bwrap|docker|none` flag, default `auto`.
8. **Billing Guardrails** — `--max-spend <USD>` flag; mid-run pause when cap is hit; resume requires explicit user OK.
9. **Provider Routing / BYOK** — multi-provider with fallback chain configured in `/Users/gdc/.deadreckon/config.toml`.
10. **Workspace Inventory & Run Queue** — `deadreckon list/attach/kill/resume` over `/Users/gdc/.deadreckon/runstate/`.

## V0 prescribed decisions (do not deliberate)

These resolve the open options that would otherwise belong in a design doc.

- **Sandbox primary.** Platform-native: `sandbox-exec` (Seatbelt) on macOS, `bubblewrap` (`bwrap`) on Linux. Flag `--sandbox auto|sandbox-exec|bwrap|docker|none`; default `auto` (auto-selects the platform-native backend). Docker is `--sandbox docker` opt-in for users who want it. Lima / Firecracker / E2B / Landlock are V1 candidates. No `bollard` dep — backends shell out via `tokio::process::Command`.
- **Plugin / skill language.** Markdown skills, Claude-Code-style. Loaded at runtime from `/Users/gdc/deadreckon/skills/`. WASM plugins are V1.
- **State store.** Plain JSON files, atomically written via temp+rename. SQLite cache is V1.
- **TUI framework.** `ratatui` + `crossterm`.
- **Provider abstraction.** Custom `Provider` trait per provider; one OpenAI-compatible shim that handles any compliant endpoint (lets users plug in OpenRouter, llama.cpp).

## Source tree layout (`/Users/gdc/deadreckon/`)

```
/Users/gdc/deadreckon/
├── Cargo.toml                          # workspace
├── DESIGN.md                           # phase-0 artifact
├── README.md                           # how to install + run
├── demo.cast                           # asciinema of end-to-end smoke
├── crates/
│   ├── deadreckon-core/                # state, locks, phase engine, snapshots
│   ├── deadreckon-providers/           # BYOK router (Anthropic, OpenAI, openai-compat)
│   ├── deadreckon-sandbox/             # backends: sandbox-exec / bwrap / docker / none
│   └── deadreckon/                     # binary (clap CLI + TUI)
├── skills/                             # markdown skills loaded at runtime
│   └── default-coding/SKILL.md
└── tests/                              # workspace integration tests
```

## Runtime state layout (`/Users/gdc/.deadreckon/`)

```
/Users/gdc/.deadreckon/
├── config.toml                         # BYOK keys, default provider, defaults
├── runstate/
│   └── <scope>/
│       ├── current/<task>.json         # active run pointer per task
│       └── runs/<run-id>/
│           ├── state.json              # PipelineState
│           ├── snapshots/turn-<N>/     # per-turn file snapshots
│           ├── provenance.jsonl        # one line per file change
│           ├── spend.jsonl             # one line per turn cost
│           ├── traces.jsonl            # LLM + tool call traces
│           └── working/                # in-progress task workdir
├── locks/<task>.lock                   # active build locks
└── library/<scope>/<run-id>/           # archived completed runs
```

## CLI surface (must implement in V0)

- `deadreckon run <goal> [--max-spend <USD>] [--sandbox auto|sandbox-exec|bwrap|docker|none] [--provider anthropic|openai|...] [--skill <name>]`
- `deadreckon list [--scope <scope>]` — list runs (active + recent).
- `deadreckon attach <run-id>` — open the live TUI on an in-flight or completed run.
- `deadreckon kill <run-id>` — terminate cleanly, release lock.
- `deadreckon resume <run-id>` — re-attach after a crash or pause; respects the phase machine.
- `deadreckon undo [--run <run-id>] [--turn <N>]` — restore files to N turns ago.
- `deadreckon show <run-id> [--turn <N>]` — print state, files changed, provenance.
- `deadreckon import {claude-code|codex|cursor}` — read-only import of histories.
- `deadreckon doctor` — preflight: platform sandbox binary reachable (`sandbox-exec` on mac, `bwrap` on Linux), Docker present only if `--sandbox docker` was used, config present, providers credentialed. Output is **actionable**: each missing item names the exact fix command.
- `deadreckon init` — first-run wizard. Interactively gathers ≥ 1 provider key (Anthropic / OpenAI / OpenAI-compatible URL + key), sets a default `--max-spend`, writes `/Users/gdc/.deadreckon/config.toml`, runs `doctor`, prints next steps. Re-runnable; merges over existing config.
- `deadreckon config get <key>` / `deadreckon config set <key> <value>` — non-interactive config edits. Keys: `providers.anthropic.api_key`, `providers.openai.api_key`, `defaults.max_spend`, `defaults.sandbox`, etc.

## User-friendliness (V0 commitments — do not violate)

User-friendliness is a V0 requirement, not a V1 polish item. The build agent
must budget UX work into every relevant phase. These commitments override
engineering-elegance preferences when they conflict.

### 1. First-run experience is one command

A new user reaches their first successful run in three commands:

```zsh
deadreckon init                              # interactive: pick provider, paste key, set defaults
deadreckon run "hello-world in rust"         # respects defaults from init
deadreckon attach <run-id>                   # live TUI
```

No manual `config.toml` editing required. `init` is the only acceptable
first-time path.

### 2. Errors include a one-line fix hint

Every `DeadreckonError` variant carries a `hint: &'static str` field rendered
after the message. A missing API key surfaces as:

```
error: Anthropic API key not configured
  hint: run `deadreckon config set providers.anthropic.api_key <KEY>`
        or `deadreckon init` to walk through setup
```

No bare HTTP status codes, no stack traces in user-facing output. Stack
traces go to `tracing` logs only (`--verbose` reveals them).

### 3. Sensible spend default with explicit confirmation above

- `--max-spend` defaults to `$10` if not specified.
- Running without `--max-spend` on a fresh session prints a one-line notice
  the first time: `using default --max-spend $10 (override with --max-spend or
  in config defaults.max_spend)`.
- Setting `--max-spend > $50` requires `--i-know-its-a-lot` or a `y/N`
  confirmation prompt (with `--no-confirm` to override in scripts).

### 4. `doctor` output is actionable

Each line is either `✓ <thing>` or `✗ <thing>` followed by the exact fix.

```
✓ sandbox-exec found at /usr/bin/sandbox-exec
✗ Docker not found
    fix: install Docker Desktop (https://docker.com/get-started)
         or run with --sandbox sandbox-exec (default on macOS)
✓ /Users/gdc/.deadreckon/config.toml present
✗ no Anthropic key configured
    fix: deadreckon config set providers.anthropic.api_key <KEY>
✓ ~250 MB free in /Users/gdc/.deadreckon (heads-up at 50 MB)
```

### 5. Quickstart in README, top of page

`/Users/gdc/deadreckon/README.md` opens with a "Quickstart" section that runs
in under two minutes for a user with an Anthropic key. The three-command flow
from §1 plus one expected-output block.

### 6. TUI visual feedback

`ratatui` view uses color and motion to communicate state, not just text:

- **Spend meter** — green 0–50% of cap, yellow 50–80%, red 80–100%, magenta
  if the run paused at cap.
- **Context meter** — same color thresholds against the model's context
  window.
- **Tool-call streaming** — each tool call rendered as a one-line entry that
  fills in real time (tool name → args → result preview).
- **Detach** — `Ctrl-D` detaches cleanly without killing the run; the TUI
  status bar reminds the user how to detach at all times.
- **Pause-at-cap** — when `--max-spend` is hit, the TUI prints the resume
  command and waits.

### 7. Sane defaults everywhere

The user should rarely need to specify a flag for the happy path.

- `--sandbox` defaults to `auto` (platform-native).
- `--max-spend` defaults to `$10`.
- `--provider` defaults to the highest-credentialed provider in config.
- `--skill` defaults to `default-coding`.
- Run ID generation is automatic; user never types one until they want to
  `attach` / `resume` / `undo`.

### 8. Time-to-first-output ≤ 5 seconds

From `deadreckon run` to first streamed token (or first tool call) in the
TUI: under 5 seconds on a warm machine. State init, lock acquire, sandbox
setup, and provider auth must not block first output.

## House style (do not violate)

- Rust 2024 edition. `cargo fmt` + `cargo clippy -- -D warnings` clean on every commit.
- No panics in library code; return `Result<_, DeadreckonError>` with `thiserror`.
- Structured logging only — `tracing::{info,warn,error}!`. No `println!` outside the binary's user-facing output.
- "Why" comments only — explain non-obvious decisions, not what the code does.
- Conventional commits per phase: `feat(core): ...`, `feat(providers): ...`, `feat(sandbox): ...`, `feat(cli): ...`, `feat(tui): ...`, `docs: ...`.
- All paths in source/config/docs absolute or anchored to `/Users/gdc/deadreckon/` (source) or `/Users/gdc/.deadreckon/` (runtime).

## Dependency policy (three tiers — do not violate)

The agent may add crates beyond the rider's named set per these tiers. The rule
of thumb between Tier 1 and Tier 2: if a future module decision would naturally
depend on the crate's presence, it is Tier 2.

### Tier 1 — free, log in commit message

Pure utility crates that do not introduce a new architectural surface. Add as
needed; mention in the commit that introduces them. No further paperwork.

Examples: `anyhow`, `thiserror`, `tempfile`, `uuid`, `chrono`, `walkdir`,
`regex`, `base64`, `hex`, `dirs`, `tar`, `zstd`, `once_cell`, `bytes`,
`humantime`, `indicatif` (CLI progress bars used only in non-TUI paths).

### Tier 2 — add, log, document

New architectural surface not already named in the rider. Add the crate, log in
the commit message with a one-line rationale, and append a row to
`/Users/gdc/deadreckon/DEPENDENCIES.md` with `{ crate, version, purpose,
alternatives_rejected }`.

Examples: a JSON-Patch crate, a TOML diff crate, a new file watcher
(`notify`), a SQLite driver added in V0 (deviation), an alternate auth flow
crate.

### Tier 3 — blocked, ask the user

Crates that contradict a prescribed decision or violate an invariant. Stop and
write the proposal to `/Users/gdc/deadreckon/CHANGELOG.md` under
`## Pending User Decision`; do not add until the user approves.

Blocked classes:

- Replacing a prescribed crate: alternatives to `tokio`, `ratatui`,
  `crossterm`, `reqwest`, `serde`, `clap`, `tracing`, `nix`, `fs2`, `which`.
- Adding a second TUI framework, second async runtime, or second HTTP client.
- Telemetry / analytics / phone-home SDKs (Sentry, Honeycomb, Datadog,
  PostHog, etc.) — local-first invariant.
- GPL / AGPL surface.
- Crates that pull > 10 MB into the release binary.
- Crates with last-publish > 24 months ago without a clear maintenance signal.

### DEPENDENCIES.md row format

```
| crate | version | tier | purpose | alternatives_rejected | added_in_commit |
```

## Engineering invariants (do not violate)

- **No edits outside** `/Users/gdc/deadreckon/` and `/Users/gdc/deadreckon/docs/goals/`.
- **No `git push`.** Phased local commits only.
- **Dependencies follow the policy above.** Tier 1 free with log; Tier 2 logged + DEPENDENCIES.md row; Tier 3 blocked pending user approval.
- **No kitchen-sink features.** Anything not in the must-have list goes to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` (notes only).
- **BYOK only.** The harness never bills the user.
- **Anti-self-attestation.** The agent cannot mark a gate green; gates are validated by the binary against the run identity.
- **Sandbox by default.** `deadreckon run` without `--sandbox` defaults to `auto` (sandbox-exec on macOS, bwrap on Linux). `--sandbox none` requires an explicit `--unsafe` flag in V1 (V0 just warns). On Linux without `bwrap` installed, doctor prints the install hint and `auto` falls back to `none` with a warning.
- **Local-first.** No SaaS sync, no cloud telemetry, no phone-home.

## V1 candidates (planned — file ideas here during V0)

Anything the build agent encounters in V0 that fits these buckets goes to
`/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` (one bullet per idea, with
the originating commit). Cite REPORT.md need numbers where they apply.

### Harness primitives

- **Sub-agent forking with context isolation.** Expose AS-BUILT §10's fork pattern as `deadreckon fork <run-id> --prompt "..."` plus auto-fork for research-heavy turns. Without it, multi-hour runs exhaust context.
- **Hook system.** Pre/post tool-call hooks loaded from `/Users/gdc/deadreckon/hooks/`. REPORT.md #15.
- **First-class MCP client.** `deadreckon mcp list/add/remove/test`. V0 consumes MCP servers; V1 exposes the surface. REPORT.md #12.
- **Time-boxed turns.** `--max-turn-seconds <N>` companion to `--max-spend`.
- **Cost-aware provider routing.** Beyond V0's fallback chain: route by task class. REPORT.md #16.
- **Spend forecasting.** Pre-run probe + recommended `--max-spend`.
- **Permission boundaries beyond the sandbox.** Per-tool allowlists / denylists. REPORT.md #11.
- **Structural verification before agent claims.** Forced grep/AST checks before the agent says "done." REPORT.md #14.
- **Execution-efficiency evals.** Trace-level efficiency scoring beyond raw JSONL. REPORT.md #24.

### TUI expansions

- Multi-run dashboard (V0 is one-run-attach).
- Diff / review pane for accepting / rejecting agent edits per turn.
- Kanban-style workspace view. REPORT.md #25.
- Real-time multi-cursor presence for collaborative sessions.

### Collaboration / handoff

- **Run handoff and sharing.** `deadreckon export <run-id> --to tar.zst` + `import <tar>` + optional `push --to <remote>`. REPORT.md #17.
- **Headless / batch mode.** `deadreckon run-batch --prompt-file <f> --report-json <out>` for CI integration.
- **Live bridging cross-tool sync** (write-back, beyond V0's read-only import).
- **Cloud sync of histories** (BYOK preserved; opt-in only).
- **Approval workflows beyond `--max-spend`** (review gates, multi-step approvals).
- **Team onboarding generation from real sessions.** REPORT.md #13.
- **Versioned team prompt libraries.** REPORT.md #23.
- **AI review limits and paid review continuity.** REPORT.md #20.
- **Meeting action items connected to code work.** REPORT.md #22.

### Sandbox / isolation

- Lima (Linux VMs on macOS).
- Firecracker microVMs.
- E2B / Modal / other hosted sandboxes.
- Landlock + seccomp (kernel-level Linux).
- Richer port / env isolation for parallel agents. REPORT.md #18.
- `--sandbox none` gated behind explicit `--unsafe`.

### Storage / state

- WASM plugins (`wasmtime`).
- SQLite cache for cross-episode queries.
- Search / embeddings over history.
- Audit / governance receipts (compliance-grade). REPORT.md #19.

### Voice / meeting

- Voice / meeting capture.

## Out of scope (forever — invariants, do not violate)

- Hosted billing or metering on behalf of the user (BYOK invariant).
- Phone-home telemetry / analytics SDKs (local-first invariant).
- GPL / AGPL surface in core dependencies.
- Authoring an MCP server framework (we consume MCP, we do not author the protocol).
- Desktop GUI (TUI + CLI only).

## V2+ (deferred past V1)

- Skills marketplace / discoverability registry.
- Cloud telemetry export to user-owned OTLP / Honeycomb / Tempo / Datadog sinks (user-controlled destinations only; never deadreckon-hosted).

## Verification helpers (zsh)

```zsh
REPO=/Users/gdc/deadreckon
STATE=/Users/gdc/.deadreckon

# Build / lint / test
cd "$REPO" && cargo build --release \
  && cargo test --workspace \
  && cargo clippy --workspace -- -D warnings \
  && cargo fmt --check

# Required artifacts
test -f "$REPO/DESIGN.md"      || echo FAIL_DESIGN_MISSING
test -f "$REPO/demo.cast"      || echo FAIL_DEMO_MISSING
test -d "$REPO/skills"         || echo FAIL_SKILLS_DIR_MISSING

# User-friendliness checks
grep -q "^## Quickstart" "$REPO/README.md"        || echo FAIL_NO_QUICKSTART
grep -qE "fn init|pub.*init.*Cmd" "$REPO/crates/deadreckon/src" -r \
                                                  || echo FAIL_NO_INIT_CMD
grep -q "hint:" "$REPO/crates/deadreckon-core/src/error.rs" \
                                                  || echo FAIL_ERRORS_NO_HINTS
grep -qE "default_value.*= ?\"10\"|max_spend.*= ?10" "$REPO/crates/deadreckon/src" -r \
                                                  || echo FAIL_NO_DEFAULT_SPEND

# Each must-have need has a code presence
for need in "Live Context" "Multi-Agent" "Infinite Undo" "Provenance" \
            "Cross-Tool" "Observability" "Sandbox" "Billing Guardrail" \
            "Provider Routing" "Workspace Inventory"; do
  grep -rq "REPORT.md: $need" "$REPO/crates" \
    || echo "FAIL_MISSING_NEED: $need"
done

# Smoke (requires sandbox-exec on mac OR bwrap on Linux + ≥ 1 provider key in /Users/gdc/.deadreckon/config.toml)
"$REPO/target/release/deadreckon" doctor
"$REPO/target/release/deadreckon" run "make a tiny rust project that prints hello" \
  --max-spend 5 --sandbox auto
"$REPO/target/release/deadreckon" list
RUN=$(ls -t "$STATE/runstate/"*/runs/ | head -1)
"$REPO/target/release/deadreckon" undo --run "$RUN"
"$REPO/target/release/deadreckon" show "$RUN"
```

## Process invariants

- **Phased local commits only.** One commit per phase boundary. No `git push` without explicit user confirmation.
- **Surface contradictions.** If this rider, AS-BUILT, REPORT, or Claude Code source disagree, stop and report rather than picking silently.
- **Per-phase progress line.** After each phase: `<phase>: <verified> | remaining: <list> | blockers: <list>`.
- **Implementation log.** Before stopping, append `/Users/gdc/deadreckon/CHANGELOG.md` with per-phase summary, exact SHAs, files touched, follow-ups.
