# deadreckon V0 Design

## Product

`deadreckon` is a Rust 2024 CLI harness for unattended long-running coding tasks. The default user flow is `deadreckon run <goal>`: create durable run state, select a BYOK provider route, execute turns in a disposable sandbox, write spend/provenance/traces/snapshots after every turn, and make the run attachable, resumable, killable, inspectable, and undoable.

The V0 implementation is intentionally local-first. Runtime state defaults to `/Users/gdc/.deadreckon/`, and tests or explicit smoke runs can override that with `DEADRECKON_HOME=/Users/gdc/deadreckon/.deadreckon-smoke` so this build process does not write outside the allowed implementation tree.

## Reference Patterns

- AS-BUILT §3: two-layer split. Rust owns state, locks, sandboxes, provider routing, and gates. Markdown skills under `/Users/gdc/deadreckon/skills/` own agent-facing instructions.
- AS-BUILT §4: `state.json` per run, status enum, gap-numbered phase IDs, and `current/<task>.json` run pointers.
- AS-BUILT §6: JSON lock records with PID liveness and heartbeat stale reclaim.
- AS-BUILT §7-8: atomic promotion/gate markers are written by binary-owned verification, not by model prose.
- AS-BUILT §9: per-turn snapshots make bounded rollback possible.
- Claude Code mining:
  - `/Users/gdc/claude-code-source-code/src/tools/BashTool/bashPermissions.ts` and `shouldUseSandbox.ts` inform command side-effect boundaries and sandbox warning posture.
  - `/Users/gdc/claude-code-source-code/src/tools/shared/spawnMultiAgent.ts` informs inherited run context for future sub-agent work; V0 records scope and session lineage but leaves explicit forks to V1.
  - `/Users/gdc/claude-code-source-code/src/upstreamproxy/upstreamproxy.ts` informs fail-open provider/proxy setup and child-process environment merging.
  - `/Users/gdc/claude-code-source-code/src/ink/renderer.ts` informs ratatui attach rendering: bounded frames, no uncontrolled terminal state.
  - `/Users/gdc/claude-code-source-code/docs/prompts/` informs the Markdown skill shape in `skills/default-coding/SKILL.md`.

## Workspace

Source lives under `/Users/gdc/deadreckon/`:

- `crates/deadreckon-core`: run paths, phase machine, JSON state, locks, heartbeats, snapshots, provenance, spend, traces, gates, imports.
- `crates/deadreckon-providers`: BYOK config at `/Users/gdc/.deadreckon/config.toml`, provider trait, Anthropic, OpenAI, OpenAI-compatible, `cli:claude-code`, `cli:codex`, and explicit `--smoke` scripted adapters, fallback routing, spend estimates.
- `crates/deadreckon-sandbox`: `sandbox-exec`, `bwrap`, `docker`, and `none` backends using `tokio::process::Command`; default `auto`.
- `crates/deadreckon`: clap CLI, ratatui attach UI, run/list/attach/kill/resume/undo/show/import/doctor.
- `skills/default-coding/SKILL.md`: Markdown skill loaded at runtime.
- `tests/`: workspace integration tests.

## Top-10 Need Mapping

1. Live Context & Spend Meter: `spend.jsonl` in `deadreckon-core`, spend summaries in `deadreckon attach`.
2. Multi-Agent Worktree Coordination: scope hashing, run pointers, and locks in `deadreckon-core`.
3. Infinite Undo For Agent Edits: per-turn `snapshots/turn-<N>/` plus `deadreckon undo`.
4. Prompt-To-Code Provenance Audit Trail: `provenance.jsonl` and `deadreckon show`.
5. Cross-Tool State Sharing: read-only import from `/Users/gdc/.claude/projects/`, `/Users/gdc/.codex/sessions/`, `/Users/gdc/.cursor/chats/`.
6. Agent Observability: `traces.jsonl` with LLM/tool/sandbox events and latencies.
7. Disposable Sandboxes: platform-native sandbox backends, Docker opt-in, `none` warning.
8. Billing Guardrails: `--max-spend <USD>` pauses runs when durable spend reaches cap.
9. Provider Routing / BYOK: three adapters and fallback chain configured from TOML.
10. Workspace Inventory & Run Queue: `list`, `attach`, `kill`, `resume`, and state scan commands.

## V0 Execution Model

The primary path is provider-driven: `deadreckon run <goal>` creates state, calls `ProviderRouter::complete`, parses model actions, executes tool calls in the configured sandbox, feeds results into history, and repeats until a provider returns `done`, a spend cap pauses the run, or the run is killed. HTTP providers use user-supplied keys or env vars; CLI providers use local subscription CLIs and run through the deadreckon sandbox wrapper.

For keyless local verification only, `deadreckon run --smoke <goal>` selects the `smoke` provider explicitly. That scripted provider still goes through the same turn loop, sandbox dispatch, snapshots, spend records, traces, provenance, and external `dr-gate` acceptance marker; it is not the default run path.

The default skill is Markdown and external to the binary. The binary loads it and records the skill name/path in state; it does not parse or mutate skill internals.

## Decisions And Conflicts

- Runtime writes during verification: the rider prescribes `/Users/gdc/.deadreckon/`, while the build instruction forbids edits outside `/Users/gdc/deadreckon/` and `/Users/gdc/stoa/docs/goals/`. The binary defaults to `/Users/gdc/.deadreckon/`, but all repository verification uses `DEADRECKON_HOME=/Users/gdc/deadreckon/.deadreckon-smoke`.
- Sub-agent forking: the architecture requires the pattern, but the V1 list explicitly moves `deadreckon fork` to V1. V0 records scope/session lineage and keeps the subprocess skill boundary; explicit multi-agent forks are documented in `docs/V1-CANDIDATES.md`.
- Sandbox fallback: on macOS `auto` selects `sandbox-exec` if available. If not available, or on Linux without `bwrap`, `doctor` reports the missing binary and `run` falls back to `none` with a warning, matching the rider.
- Live Tier C CLI verification: `claude` and `codex` are present on this machine, but running either can write subscription-tool session state outside `/Users/gdc/deadreckon/`. The checked-in verification therefore uses fake CLI binaries plus sandbox-wrapped provider tests unless the user explicitly approves a live subscription run.

## Verification

Final verification target:

```bash
cd /Users/gdc/deadreckon
cargo build --release
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check
DEADRECKON_HOME=/Users/gdc/deadreckon/.deadreckon-smoke ./target/release/deadreckon run "tiny hello rust" --smoke --sandbox none --max-spend 1
```

`demo.cast` records real release-binary output from the explicit keyless smoke path without relying on provider credentials.
