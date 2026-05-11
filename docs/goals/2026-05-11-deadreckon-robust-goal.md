GOAL: Harden deadreckon — replace scaffolding with robust implementations.

The earlier build shipped a credible harness at `/Users/gdc/deadreckon/` (primary loop, providers, mock-driven verification all real), but the retrospective flagged ten thin areas that are scaffolding rather than finished systems. Deepen each. **No new features** — depth, not breadth. Drop "V0" nomenclature; the product is just deadreckon at the `alpha` tier. Stop when the rider's robustness matrix passes and prior invariants hold.

**What "robust" means.** Each thin area gets a targeted depth test that exercises real edge cases (mid-turn kill, partial traces, concurrent runs, adversarial prompts) and would have caught the scaffolding-thin behavior. Happy-path-only tests don't count. Out of scope: V1 pain points (sub-agent forking CLI, hooks, MCP client, search/embeddings, cost-aware routing).

**References — read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-05-11-deadreckon-robust-rider.md` — area contracts, test names, V0-cleanup grep list.
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-deadreckon-primary-flow-rider.md` + `2026-05-10-deadreckon-build-rider.md` — prior invariants hold.
- `/Users/gdc/deadreckon/{CHANGELOG.md,DESIGN.md,docs/GAP-ANALYSIS.md}`.
- `/Users/gdc/Downloads/AS-BUILT-ARCHITECTURE.md` §6 / §7 / §8 / §11 / §15 / §17.
- `/Users/gdc/claude-code-source-code/src/{tools,coordinator,upstreamproxy}/`.

**Phase plan — commit locally each boundary; no `git push`. One phase per thin area.**

1. **TUI streaming.** Broadcast channel from run loop → `ratatui`; live tool-call events; per-turn timer; clean detach.
2. **Resume partial trace.** Reconstruct when `traces.jsonl` ends mid-tool-call; `--from-turn N` override.
3. **Cancellation.** Hierarchical `CancellationToken` (run → turn → tool → child); HTTP aborts; subprocess SIGTERM→SIGKILL. Storm test: 10 starts + 10 kills.
4. **Wall-clock spend for CLI.** Per-CLI-turn `wall_time_seconds`; `--max-wall-seconds`; subscription→wall-time mapping in config.
5. **Sandbox hardening.** Per-run Seatbelt / bwrap profiles: fs allowlist, network allowlist, tmpfs `$HOME`. Adversarial test reads `~/.ssh/` blocked.
6. **Doctor exhaustive.** Provider-ping (cheapest model), disk space, write perms, sandbox versions, OS/kernel sanity. Each line actionable.
7. **Import normalization.** Parse Claude/Codex JSONL + Cursor SQLite into deadreckon trace + provenance. Round-trip test.
8. **Acceptance spec.** `dr-gate` reads `acceptance.yaml` (tests, file-exists, content-match, build-success); self-attest attempt refused.
9. **Multi-run coordination.** Document lock ordering; test ≥ 2 concurrent runs in different scopes (no bleed) and same scope (refused with hint).
10. **Promotion / library.** `working/<run>` → `library/<run-id>/` atomic swap + manifest, lock-protected, post-acceptance. Crash-between-steps recoverable.

**Drop V0 nomenclature.** Edit `/Users/gdc/deadreckon/` per rider's cleanup list: README/DESIGN/CHANGELOG/source comments. `Cargo.toml` → `version = "0.1.0"`. "V0" stays only in dated retrospective entries.

**Verification.**

- `cd /Users/gdc/deadreckon && cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`.
- Per-area depth tests (rider matrix) pass.
- Stress: 5 concurrent runs in different scopes for 10 min — no lock leaks, no state bleed.
- Adversarial: sandbox-escape prompt fails to read disallowed paths; forged self-attest marker refused.
- `grep -rE "V0|v0-scaffolding" /Users/gdc/deadreckon/{crates,README.md,DESIGN.md}` empty (CHANGELOG retrospective entries allowed).
- All prior rider invariants pass.
- No `git push`.

**Checkpoints.** Each phase: verify, commit locally, write one progress line (phase, verified, remaining, blockers). Conflicts to `GAP-ANALYSIS.md`.

**Stop when:** ten areas have depth-tests passing, stress + adversarial gates green, V0 nomenclature removed, prior invariants hold.
