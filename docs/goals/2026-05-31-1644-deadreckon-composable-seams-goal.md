GOAL: Decompose deadreckon's governance core into **composable seams**. Today `policy`, `model-catalog`, `hooks`, and the `event-sink` are compiled in — change any and you fork the Rust (the monolith trap iii's harness post names). deadreckon already externalizes two (`cli:*` providers, `dr-gate`); generalize into **one** seam contract so those four become swappable from `config.toml`, keep the gate deliberately **non-swappable** (a swappable trust root is forgeable), and close the unbounded-history gap on the direct-API path. Headline word: **Composable**.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-05-31-1644-deadreckon-composable-seams-rider.md` — the contract: primitive, wire shapes, fail policies, depth tests.
- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` — §13 gate, §35 tamper, §38 layout.
- `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/turn_loop.rs` — `run_turn_loop`; insertion points; L775 history join.
- `/Users/gdc/deadreckon/crates/deadreckon-providers/src/{types.rs,router.rs,cli_common.rs,registry/mod.rs,config.rs}` — `Provider`, router, subprocess contract, `ModelEntry`.
- `/Users/gdc/deadreckon/crates/deadreckon-core/src/{gate.rs,events.rs}` + `.../bin/dr-gate.rs` — trust root; `RunEventKind`.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` — invariants hold.

**Posture.** Production-release track. Files-not-fields: seam wiring is a new `[seams]` table in `config.toml`; per-run audit is new files (`seams.json`, `compaction.jsonl`). No `PipelineState`/`Plan`/`AcceptanceMarker`/`ProviderEntry` schema changes. The **gate is not a seam**; built-in telemetry, `events.jsonl`, and `dr-gate` are untouched. No `git push`. Edits inside `/Users/gdc/deadreckon/`. Larger designs → `docs/V1-CANDIDATES.md`.

**One primitive.** A `SeamCommand`: run the configured command **sandboxed**, JSON on stdin → JSON on stdout, with a timeout and a per-kind fail policy. No command ⇒ the built-in default runs, behavior identical. Install/remove one to slide thin↔thick; `--no-seams` forces all built-ins.

**Seams + the gap.**

- **policy** — per tool-call `{function_id,command,working_dir}`→`{allow|deny}`, **fail-closed**; the sandbox stays the floor (seams only narrow it).
- **catalog** — returns `context_window`/pricing; **fail-open** ⇒ built-in `ModelEntry` list.
- **hooks** — observe-only fanout of tool events; can't change the decision; non-fatal.
- **event-sink** — additive mirror of `RunEvent`; `events.jsonl` stays source of truth (attach unaffected); non-fatal.
- **compaction** — direct-API path only: deterministic context-window elision keyed to catalog `context_window`, logged to `compaction.jsonl`, never dropping the goal/acceptance spec; resume-identical.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `cargo test -p <crate> && cargo fmt --check` green → conventional local commit → CHANGELOG. P1 lands the primitive + gate-has-no-seam guard (RED). P11: AS-BUILT §39 + CHANGELOG + deferrals.

**Verification.**

- Every rider depth test present and passing; `cargo fmt --check`; `git diff --check`.
- Smoke (`--smoke --sandbox none`): a `deny` policy seam blocks a tool call; `--no-seams` restores built-in behavior; a malformed `catalog` seam falls back without blocking.
- Smoke: no `[seams]` key can target the gate; no seam can write/redirect the marker or read `gate/nonce`.
- Smoke: a long direct-API run compacts deterministically while keeping the goal/acceptance spec; the CLI path is never compacted.
- No edits outside the repo; no `git push`; no durable-schema changes.

**Stop when** the four seams are swappable via `[seams]`/`--no-seams` with their fail policies, the gate stays non-swappable (trust tests green), direct-API history compacts deterministically without dropping the spec, AS-BUILT §39 + CHANGELOG + V1-CANDIDATES updated, work committed locally.
