GOAL: Build V0 of deadreckon at `/Users/gdc/deadreckon/`.

Ship a Rust agentic CLI named **deadreckon** whose **primary** flow is unattended long-running coding tasks with BYOK across providers (opencode-style), addressing the top 10 unmet needs in `/Users/gdc/stoa/docs/research/2026-05-10-unmet-needs/REPORT.md`. The harness adopts the two-layer pattern from `/Users/gdc/Downloads/AS-BUILT-ARCHITECTURE.md` and mines `/Users/gdc/claude-code-source-code/` for tool/permission/TUI patterns. Implementation lives at `/Users/gdc/deadreckon/`; do not edit anything outside `/Users/gdc/deadreckon/` and `/Users/gdc/deadreckon/docs/goals/`. Stop only when verification passes; add crates per the rider's dependency policy.

**What you're building.** Cargo workspace at `/Users/gdc/deadreckon/`: crates `deadreckon-{core,providers,sandbox}` + `deadreckon` binary (`run/list/attach/kill/resume/undo/show/import` + `ratatui` TUI). Rider prescribes the 10 must-have needs and V0 decisions.

**References — read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-05-10-deadreckon-build-rider.md` — architecture, must-have needs, decisions, layout, verification.
- `/Users/gdc/stoa/docs/research/2026-05-10-unmet-needs/REPORT.md` — 25 needs; top 10 are V0 scope.
- `/Users/gdc/Downloads/AS-BUILT-ARCHITECTURE.md` — Printing Press pattern. Port structure, not Go code.
- `/Users/gdc/cli-printing-press/internal/pipeline/{state,lock,phase5_gate}.go` + `/Users/gdc/cli-printing-press/skills/printing-press/SKILL.md` — reference shapes.
- `/Users/gdc/claude-code-source-code/` — mine `src/{assistant,tools,coordinator,upstreamproxy,ink}/` + `docs/prompts/` for tool loop, permission model, provider proxy, TUI patterns.
- `/Users/gdc/deadreckon/{README.md,DESIGN.md,CHANGELOG.md}` — current repo conventions.

**Phase plan — commit locally in `/Users/gdc/deadreckon/` each boundary; no `git push`.**

0. **DESIGN.md.** Write `/Users/gdc/deadreckon/DESIGN.md` summarizing rider architecture + 10 needs + V0 decisions.
1. **Bootstrap.** `cargo init` workspace at `/Users/gdc/deadreckon/`; four crates per rider; wire `tracing/tokio/serde/clap`. `cargo build` clean.
2. **State engine.** `PipelineState` + `state.json`, gap-numbered phases, run pointer, scopes. Resume-after-`kill -9` test.
3. **Locks.** File locks + heartbeats + PID liveness probe. Stale-lock reclaim test.
4. **Provider router.** BYOK at `/Users/gdc/.deadreckon/config.toml`; three adapters; fallback; per-turn spend.
5. **Sandbox.** `sandbox-exec` (mac) / `bwrap` (Linux) per run; Docker opt-in. Isolate fs/net/env.
6. **Run loop.** `deadreckon run <goal>` long-task driver with per-turn snapshots + provenance.
7. **Verbs.** `deadreckon list/attach/kill/resume/undo/show`.
8. **TUI.** `ratatui` live status: spend meter, context meter, recent turns, tool calls.
9. **Cross-tool import.** Read-only `/Users/gdc/.{claude/projects,codex/sessions,cursor/chats}/`.
10. **Polish & verify.** End-to-end smoke; asciinema cast at `/Users/gdc/deadreckon/demo.cast`.

**Verification.**

- `cd /Users/gdc/deadreckon && cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`.
- Smoke: `deadreckon run "tiny hello rust"` completes in sandbox with provenance + spend + snapshots; `undo/list/attach/kill/resume` work.
- Each of the 10 must-have needs maps to ≥ 1 module or command (rider grep check).
- `/Users/gdc/deadreckon/DESIGN.md` and `/Users/gdc/deadreckon/demo.cast` exist.
- No edits outside `/Users/gdc/deadreckon/` and `/Users/gdc/deadreckon/docs/goals/`.
- No `git push`, no kitchen-sink features beyond rider scope.

**Checkpoints.** Each phase boundary: run verification, commit locally with conventional commit, write one progress line (phase, verified, remaining, blockers). If rider conflicts with code, evaluate, make a decision, log it in docs and continue. 

**Stop when:** verification passes, DESIGN.md + demo cast exist, work committed locally, no `git push`, no rider invariant violated.
