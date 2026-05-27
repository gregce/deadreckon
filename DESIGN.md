# deadreckon Alpha Design

## Product

`deadreckon` runs your coding agent unattended, and a separate watchdog process, not the agent, decides when the work is actually done. It is a Rust 2024 CLI harness that supervises agent CLIs (Claude Code, Codex, Gemini CLI, GitHub Copilot CLI, OpenCode, Pi) and BYOK API routes rather than replacing them. The default user flow is `deadreckon run <goal>`: create durable run state, select a provider route, execute turns in a disposable sandbox, write spend/provenance/traces/snapshots after every turn, gate completion with a signature the agent process cannot forge, and make the run attachable, resumable, killable, inspectable, and undoable. The output is an auditable artifact, not a chat transcript.

The alpha implementation is intentionally local-first. Runtime state defaults to `/Users/gdc/.deadreckon/`, and tests or explicit smoke runs can override that with `DEADRECKON_HOME=/Users/gdc/deadreckon/.deadreckon-smoke` so this build process does not write outside the allowed implementation tree.

## Reference Patterns

- AS-BUILT §3: two-layer split. Rust owns state, locks, sandboxes, provider routing, and gates. Markdown skills under `/Users/gdc/deadreckon/skills/` own agent-facing instructions.
- AS-BUILT §4: `state.json` per run, status enum, gap-numbered phase IDs, and `current/<task>.json` run pointers.
- AS-BUILT §6: JSON lock records with PID liveness and heartbeat stale reclaim.
- AS-BUILT §7-8: atomic promotion/gate markers are written by binary-owned verification, not by model prose.
- AS-BUILT §9: per-turn snapshots make bounded rollback possible.
- Claude Code mining:
  - `/Users/gdc/claude-code-source-code/src/tools/BashTool/bashPermissions.ts` and `shouldUseSandbox.ts` inform command side-effect boundaries and sandbox warning posture.
  - `/Users/gdc/claude-code-source-code/src/tools/shared/spawnMultiAgent.ts` informs inherited run context for future sub-agent work; alpha records scope and session lineage but leaves explicit forks to V1.
  - `/Users/gdc/claude-code-source-code/src/upstreamproxy/upstreamproxy.ts` informs fail-open provider/proxy setup and child-process environment merging.
  - `/Users/gdc/claude-code-source-code/src/ink/renderer.ts` informs ratatui attach rendering: bounded frames, no uncontrolled terminal state.
  - `/Users/gdc/claude-code-source-code/docs/prompts/` informs the Markdown skill shape in `skills/default-coding/SKILL.md`.

## Workspace

Source lives under `/Users/gdc/deadreckon/`:

- `crates/deadreckon-core`: run paths, phase machine, JSON state, codebase mode records, locks, heartbeats, snapshots, provenance, spend, traces, gates, imports, chains, and run docs primitives.
- `crates/deadreckon-runtime`: provider turn loop, sandboxed tool dispatch, cancellation checks, docs polish orchestration, `dr-gate` invocation, and promotion orchestration.
- `crates/deadreckon-providers`: BYOK config at `/Users/gdc/.deadreckon/config.toml`, provider trait, Anthropic, OpenAI, OpenAI-compatible, `cli:claude-code`, `cli:codex`, `cli:gemini`, `cli:copilot`, `cli:opencode`, `cli:pi`, and explicit `--smoke` scripted adapters, fallback routing, spend estimates.
- `crates/deadreckon-sandbox`: `sandbox-exec`, `bwrap`, `docker`, and `none` backends using `tokio::process::Command`; default `auto`.
- `crates/deadreckon`: clap parser, command handlers, ratatui attach UI, init/setup, config/settings, run, status/next, list/runs, doc/docs, attach/watch, kill/stop, resume/continue, undo/restore, show/inspect, import, materialize/export, finish/done, apply/keep, abandon/discard, cleanup/prune, extend/follow-up, library/artifacts, chain, and doctor/check.
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
9. Provider Routing / BYOK: adapters, CLI sub-agents, `deadreckon init`, `deadreckon config get/set`, and fallback chain configured from TOML.
10. Workspace Inventory & Run Queue: project-scoped `list`, `status`/`next`, `latest` run aliases, `attach`, `kill`, `resume`, `cleanup`, and state scan commands.

## Execution Model

The primary path is provider-driven: `deadreckon run <goal>` resolves the
codebase mode, previews the plan, creates state, and hands execution to
`deadreckon-runtime`. The runtime calls `ProviderRouter::complete`, parses model
actions, executes tool calls in the configured sandbox, feeds results into
history, writes docs, invokes `dr-gate`, and promotes successful runs. It
repeats until a provider returns `done`, a spend cap pauses the run, or the run
is killed. HTTP providers use user-supplied keys or env vars; CLI providers use
local subscription CLIs and run through the deadreckon sandbox wrapper.

In a git repo, the default codebase mode is `worktree`: deadreckon creates
`~/.deadreckon/worktrees/<scope>-<run-id>` on a `dr/...` branch, runs the agent
there, and leaves the user's checkout untouched until `deadreckon apply`. Outside
git, interactive users can initialize git, copy the directory, or cancel.
Non-interactive users must pass `--fresh`, `--from`, or initialize git first.
The former empty working directory path remains available as `--fresh`.

For keyless local verification only, `deadreckon run --smoke <goal>` selects the `smoke` provider explicitly. That scripted provider still goes through the same turn loop, sandbox dispatch, snapshots, spend records, traces, provenance, and external `dr-gate` acceptance marker; it is not the default run path.

The default skill is Markdown and external to the binary. The binary loads it and records the skill name/path in state; it does not parse or mutate skill internals.

## CLI Lifecycle

Completed artifacts are promoted into `/Users/gdc/.deadreckon/library/<scope>/<run-id>/`.
Worktree runs keep their `dr/...` branch available for review and finish with
`deadreckon finish <run-id>`, `deadreckon apply <run-id>`, or
`deadreckon abandon <run-id>`. `done` aliases `finish`, `keep` aliases `apply`,
and `discard` aliases `abandon`. `apply` supports squash, merge, and
cherry-pick strategies; `abandon` removes the worktree and, unless
`--keep-branch` is used, the temporary branch.

Copy and fresh runs use `deadreckon materialize <run-id> --dest <path>` or the
`deadreckon export <run-id> --dest <path>` alias to copy the library artifact to
a user-owned path. `deadreckon finish <run-id> --dest <path>` routes to the same
export behavior for copy/fresh runs. These commands write
`.deadreckon/parent.json` and record the reverse `.materialized-to` marker in
the library. `deadreckon extend
<run-id> "follow-up goal"` preserves the parent's mode semantics: worktree
parents create a child `dr/...` branch off the parent branch, copy/fresh parents
seed a new working tree from the parent library, and in-place parents refuse with
a direct `run --in-place` hint. Extended runs store parent lineage in
`working/.deadreckon/parent.json` and start the normal turn loop with reset
resource caps.

For normal operation, run ids accept unique prefixes and `latest` / `last`
resolve to the most recent run in the current project scope. `deadreckon list`
defaults to that current scope, `deadreckon list --all` shows global history, and
running `deadreckon` without a subcommand is equivalent to `deadreckon status`.
`deadreckon cleanup` / `deadreckon prune` remove abandoned, stale, or explicitly
selected completed worktree runs while leaving promoted library artifacts in
place.

Lineage and codebase mode metadata intentionally stay outside `PipelineState`;
show/list/hints derive them from marker files and `working/.deadreckon/codebase.json`
so the state schema remains stable.

Self-documenting run artifacts also live outside `PipelineState`. Each run
starts `working/.deadreckon/docs/`, appends deterministic per-turn records, and
attempts one `run-narrator` polish pass before promotion unless `--no-docs` is
set. Public copies land under `docs/` in the promoted artifact, and `apply`
builds its default commit body from the narrative and decisions docs.

## Decisions And Conflicts

- Runtime writes during verification: the rider prescribes `/Users/gdc/.deadreckon/`, while the build instruction forbids edits outside `/Users/gdc/deadreckon/` and `/Users/gdc/stoa/docs/goals/`. The binary defaults to `/Users/gdc/.deadreckon/`, but all repository verification uses `DEADRECKON_HOME=/Users/gdc/deadreckon/.deadreckon-smoke`.
- Sub-agent forking: the architecture requires the pattern, but the V1 list explicitly moves `deadreckon fork` to V1. Alpha records scope/session lineage and keeps the subprocess skill boundary; explicit multi-agent forks are documented in `docs/V1-CANDIDATES.md`.
- Sandbox fallback: on macOS `auto` selects `sandbox-exec` if available. If not available, or on Linux without `bwrap`, `doctor` reports the missing binary and `run` falls back to `none` with a warning, matching the rider.
- Live Tier C CLI verification: `codex` was run with `exec --ephemeral --dangerously-bypass-approvals-and-sandbox` inside deadreckon's outer `sandbox-exec` wrapper. Run `59c57e4565704135a9982789d0754803` produced `working/notes.md`, `traces.jsonl`, `provenance.jsonl`, snapshots, and a validated `dr-gate` marker without raw API keys.
- First-run UX: `deadreckon init` writes `/Users/gdc/.deadreckon/config.toml`, `deadreckon config get/set` performs non-interactive edits, `doctor` prints actionable check/fix lines, and `run` enforces the `$50` confirmation guard for scripts.
- Codebase modes: the architecture keeps mode metadata in
  `working/.deadreckon/codebase.json` rather than extending `PipelineState`.
  This preserves existing runstate readers while allowing worktree, copy,
  in-place, and fresh flows to evolve independently.
- Crate boundary: `deadreckon-core` is adapter-free and owns durable schemas and
  primitives. `deadreckon-runtime` owns the dependencies on providers and
  sandboxes, so core tests and downstream readers can reason about state without
  pulling in HTTP, subprocess, or platform sandbox orchestration.

## Verification

Final verification target:

```bash
cd /Users/gdc/deadreckon
cargo build --release
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check
DEADRECKON_HOME=/Users/gdc/deadreckon/.deadreckon-smoke ./target/release/deadreckon run "tiny hello rust" --smoke --sandbox none --max-spend 1
DEADRECKON_HOME=/Users/gdc/deadreckon/.deadreckon-smoke ./target/release/deadreckon init --provider cli:codex --max-spend 5 --sandbox sandbox-exec --no-confirm
DEADRECKON_HOME=/Users/gdc/deadreckon/.deadreckon-smoke ./target/release/deadreckon config set providers.cli-codex.kind cli-codex
DEADRECKON_HOME=/Users/gdc/deadreckon/.deadreckon-smoke ./target/release/deadreckon config set providers.cli-codex.binary codex
DEADRECKON_HOME=/Users/gdc/deadreckon/.deadreckon-smoke ./target/release/deadreckon config set providers.cli-codex.extra_args '["--ephemeral", "--skip-git-repo-check", "--dangerously-bypass-approvals-and-sandbox", "--ignore-rules"]'
DEADRECKON_HOME=/Users/gdc/deadreckon/.deadreckon-smoke ./target/release/deadreckon run "make a 5-line file at notes.md describing dead reckoning" --provider cli-codex --sandbox sandbox-exec --max-spend 5
```

`demo.cast` records real release-binary output from the live `cli:codex` path without relying on raw provider API keys.
