GOAL: Complete deadreckon's primary agentic loop. Build something REAL.

V0 scaffolding at `/Users/gdc/deadreckon/` shipped, but `deadreckon run` is a hardcoded shell-script fallback — no LLM, no tool loop. Replace with a provider-driven turn loop; add **CLI sub-agent providers** (`cli:claude-code`, `cli:codex`) for subscription-BYOK; prove via a keyless mock HTTP provider. Stop when the rider's three-tier verification passes and the original V0 invariants still hold.

**Real means.** `run <goal>` calls `ProviderRouter::complete` and iterates **turn → tool-call → sandboxed exec → result** until done / budget / killed. CLI providers: one `complete()` = one `claude`/`codex` subprocess, snapshotted around. All artifacts derive from real provider activity. `kill` interrupts within 2 s. `resume` continues from saved turn. Default `run` is never hardcoded; `--smoke` keeps the keyless fallback, labeled.

**References — read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-05-11-1400-deadreckon-primary-flow-rider.md` — mock spec, CLI invocations, test matrix, anti-self-attestation gate, verification helpers.
- `/Users/gdc/deadreckon/docs/goals/2026-05-10-1400-deadreckon-build-rider.md` — original V0 rider; invariants still hold.
- `/Users/gdc/Downloads/AS-BUILT-ARCHITECTURE.md` §8 / §9 / §10 / §17.
- `/Users/gdc/claude-code-source-code/src/{assistant,tools,coordinator,upstreamproxy}/`.
- `/Users/gdc/deadreckon/{CHANGELOG.md,DESIGN.md,crates/}`.

**Phase plan — commit locally each boundary; no `git push`.**

0. **Gap analysis.** `/Users/gdc/deadreckon/docs/GAP-ANALYSIS.md`: one row per audited hole with file:line.
1. **Mock HTTP provider.** axum-based OpenAI-compatible recorder + fixtures in `crates/deadreckon-providers/tests/`.
2. **CLI sub-agent providers.** `cli:claude-code` and `cli:codex` implement `Provider`; snapshot around subprocess; spend = subscription-flag + wall-time cap.
3. **Turn loop.** Real loop in `deadreckon-core` via `ProviderRouter::complete`; HTTP path parses tool calls, CLI path treats subprocess as one turn; loop until done / budget / killed.
4. **Supervision.** Persist child PIDs to `state.json`; `kill` cancels reqwest + `SIGTERM`/`SIGKILL`; `resume` reads history and reattaches.
5. **Anti-self-attestation gate.** Separate `dr-gate` binary writes `proofs/turn-acceptance.json`; deadreckon validates against `run_id`.
6. **Real provenance + spend.** Every `provenance.jsonl` `tool_call_id` matches a `traces.jsonl` entry.
7. **Recorded demo.** `asciinema rec` of `init → run --provider cli:claude-code → attach → kill → resume → undo → show`.
8. **Verify.**

**Verification — all tiers per rider must pass.**

- `cd /Users/gdc/deadreckon && cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`.
- **Tier A (keyless, must pass).** Mock smoke records ≥ 3 LLM requests; run produces ≥ 3 turns + ≥ 3 traces + ≥ 3 spend lines + ≥ 2 tool-driven provenance lines; kill-mid-turn cancels within 2 s and sets `killed`; resume preserves history.
- **Tier B (code shape, must pass).** `grep -r "coding_turn_script\|hardcoded_smoke" crates/` empty; `head -1 demo.cast` is asciicast v2 JSON; `cli:claude-code` + `cli:codex` modules present.
- **Tier C (live, gated on `command -v claude || command -v codex`).** Real subprocess trace in `traces.jsonl`; sub-agent file effects in `snapshots/turn-1/` + `provenance.jsonl`.
- Every `provenance.jsonl` `tool_call_id` matches `traces.jsonl`.
- Original V0 rider invariants (dependency policy, sandbox defaults, UX commitments) all still pass.
- No `git push`.

**Checkpoints.** Each phase: verify, commit locally, write one progress line (phase, verified, remaining, blockers). Conflicts logged to `GAP-ANALYSIS.md`.

**Stop when:** Tiers A+B pass; Tier C runs if `claude` or `codex` is present; recorded demo exists; original V0 invariants hold.
