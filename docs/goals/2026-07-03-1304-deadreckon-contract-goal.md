GOAL: Make the definition of done trustworthy before a dollar is spent — the harness compiles it from the run goal, forces it to test behavior, and shows it to you to accept or re-prompt. Today `deadreckon acceptance` drafts a done contract from a narrow request that never sees the run goal, through a prompt that steers toward source-scanning checks (keyword greps, `npm --if-present`), then commits it with no preview; `start` only prints "project (N checks)". The result is scope drift (a goal that says "realtime" whose contract never checks it) and a gate a keyword-only stub passes. This slice makes the compiler goal-aware and execution-oriented, adds a deterministic falsifiability lint plus one clamped critic pass, and turns the existing review into a real accept / re-prompt / edit loop surfaced on the Course card. Land this slice named Contract.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-07-03-1304-deadreckon-contract-rider.md` — compiled-contract read model, prompt spec, lint + critic rules, review loop, card DONE spec, eleven phases, depth tests.
- `crates/deadreckon/src/commands/acceptance.rs` — `acceptance_agent_prompt`, `acceptance_agent_command_in_dir`, `AcceptanceAgentMode`, the four check kinds.
- `crates/deadreckon/src/commands/start.rs` — `prompt_start_existing_done_criteria` (the review loop that exists), the Draft invocation (~`:2167`), `NOUN_DONE_CONTRACT`.
- `crates/deadreckon/src/commands/course.rs` — the course card + its DONE section.
- `docs/AS-BUILT-ARCHITECTURE.md` §13.1/§35 (Polyglot done-contract), §46 (Course); `docs/V1-CANDIDATES.md`. Prior riders hold; Helm claims §47, Contract takes §48.

**Posture.** Stable track (0.5.0). No `PipelineState`/acceptance schema changes — the contract stays `acceptance.yaml` + `.md` + helper scripts under `.deadreckon/acceptance/`; the compiled read model is a projection. Check kinds stay the four that exist — behavior is forced by the PROMPT and the lint, not new kinds. Deterministic-first: the falsifiability lint is the floor; the critic is ONE clamped provider call with at most one auto-redraft. No `git push`. Edits inside `/Users/gdc/deadreckon`. Decisions → V1-CANDIDATES.

**The contract, compiled then reviewed.**

- Goal-aware: the run goal is threaded into `acceptance_agent_prompt`; drafts derive from goal + request, and `start` reconciles goal↔contract, flagging any goal clause no check covers.
- Execution-oriented: the prompt demands checks that build→start→drive→assert and known-input→known-output; every substantive check must be able to FAIL; keyword-only scanning and `--if-present`-only build/test are banned.
- Falsifiability lint (deterministic) + one critic pass: the lint flags stub-passable checks; the critic scores goal-coverage and rejects a contract a keyword stub would pass, auto-redrafting once.
- Human in the loop: a real accept / re-prompt / edit review — each re-prompt re-runs the compiler (goal + prior draft + your note) and re-shows — surfaced at authoring AND on the card's DONE section (real checks, `d` to review, `--json` parity).

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `make verify` green → conventional-commit → CHANGELOG. P11 adds AS-BUILT §48 + V1-CANDIDATES.

**Verification.**

- Every rider depth test present and passing; `make verify` green each commit.
- A drafted contract on a fixture goal names the goal in its prompt, contains ≥1 behavioral check, and the lint rejects a keyword-only contract; a goal clause with no covering check surfaces as divergence at `start`.
- `start` renders the real checks on the card, `d` opens the accept/re-prompt/edit loop, a re-prompt re-compiles and re-shows, `--json` emits checks + divergence; `--yes`/non-TTY skips the loop but still surfaces divergence.

**Stop when** verification passes, AS-BUILT §48 + V1-CANDIDATES + a `Contract (stable)` CHANGELOG section are updated, committed locally.
