GOAL: Add `deadreckon verdict` — a glanceable "did it actually work?" report for ANY run, including runs imported from other tools (Claude Code, Codex, aider) and an `--all` comparison for several runs at once. A returning operator who left agents running unattended, or ran several in parallel, has no fast way to answer the one question that matters: did this run do the right thing, quietly break something, or just claim "done"? DeadReckon already owns every piece — cross-tool `import` (§16), the `dr-gate` re-runnable check engine, file-diff-since-snapshot, and `VerdictSurface` (one verdict + one action) — but nothing assembles them into a command that re-verifies a run NOW. This slice composes them into a read-only verb that turns "trust me, it's done" into "VERIFIED / REGRESSED / UNVERIFIED, here's the evidence, here's the one next thing." Land this slice named Verdict.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-06-25-1422-deadreckon-verdict-rider.md` — signatures, schema, depth tests, the three verdict states.
- `/Users/gdc/deadreckon/crates/deadreckon/src/verdict_surface.rs` — `VerdictSurface`, `VerdictKind`.
- `/Users/gdc/deadreckon/crates/deadreckon-core/src/gate.rs` — `evaluate_acceptance`, `validate_acceptance_marker`, `AcceptanceMarker`.
- `/Users/gdc/deadreckon/crates/deadreckon/src/commands/{import.rs,inspection.rs}` — import + `--json` envelope.
- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` §13/§16/§37; `docs/V1-CANDIDATES.md`. Prior riders hold.

**Posture.** Stable track (0.3.1). Read-only verb — no `PipelineState`/`AcceptanceMarker` schema breakage, no run-state mutation, no promotion. Re-running checks uses the existing `dr-gate`/`evaluate_acceptance` path against the run's recorded working dir; the original marker is read, never overwritten. No `git push`. Edits inside `/Users/gdc/deadreckon`. Major decisions → V1-CANDIDATES.

**Three honest states.**

- **VERIFIED** — valid signed marker AND re-running its acceptance checks now still passes.
- **REGRESSED** — a marker existed (or checks once passed) but re-running them now fails: the work silently broke.
- **UNVERIFIED** — no signed marker (imported/paused/failed run): verdict runs the declared checks fresh, labeled verified-now-not-at-build-time.

**Verb.**

- `verdict <id|latest>` — re-verify one run; print verdict + Explanation/Evidence (per-check pass/fail, changed-file summary) + one recommended command, via `VerdictSurface`.
- `verdict --all [--limit N]` — a comparison table across recent/parallel runs (id, goal, verdict, checks, spend) so "several at once" collapses to one screen.
- `--json` parity for both; non-TTY safe.

**Trust, not vibes.** Every line cites a durable artifact (the marker, `proofs/`, acceptance results, the diff), never a model claim. Re-verification is the same check engine the gate uses, so VERIFIED means the same here as at the gate. Imported runs are marked not-natively-gated.

**Friendliness.** Auto-detect (latest run if no id). Refuse with `try:` for unknown ids. One verdict, one primary action. The result is cached to `<run>/proofs/verdict-<ts>.json` for audit; never mutates run lifecycle.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `make verify` green → conventional-commit → CHANGELOG line. P11 adds AS-BUILT §13/§37 verdict subsections.

**Verification.**

- Every rider depth test present and passing; `make verify` green each commit.
- A valid-marker run whose checks still pass reports VERIFIED; mutating a covered file so a check fails flips it to REGRESSED; an imported run with no marker reports UNVERIFIED with fresh results.
- `verdict --all --json` emits a stable array; unknown id refuses with `try:`. `git diff --check` clean. No `git push`. No schema breakage.

**Stop when** verification passes, AS-BUILT + V1-CANDIDATES + a `Verdict (stable)` CHANGELOG section are updated, phases committed locally.
