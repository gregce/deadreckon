GOAL: Add Rules-as-gate — a project rules file the acceptance gate enforces, tamper-protected like every other check, so "agents ignore my coding rules" stops being true. Today an operator's conventions (no `unwrap()` in src, no `console.log`, required license headers, forbidden patterns) live only in prose the agent may or may not honor; nothing makes them a hard stop. This slice adds a new `AcceptanceCheck::Rules` variant compiled from a `deadreckon-rules.yaml`, evaluated against the run's touched files, that refuses "done" when a forbidden pattern appears or a required one is missing — and binds the rules file into the gate signature so an agent cannot pass by quietly deleting or weakening a rule. Drop a rules file, the gate enforces it. Land this slice named Rules.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-06-25-1423-deadreckon-rules-gate-rider.md` — rules schema, evaluation, tamper binding, depth tests.
- `/Users/gdc/deadreckon/crates/deadreckon-core/src/gate.rs` — `AcceptanceCheck`, `parse_acceptance_checks`, `evaluate_check`, `compiled_acceptance_checks`, `marker_signature`.
- `/Users/gdc/deadreckon/crates/deadreckon-core/src/tamper.rs` — `check_coverage`, `classify`, `touched_files`, the acceptance.yaml-protection precedent.
- `docs/AS-BUILT-ARCHITECTURE.md` §13/§35; `docs/{CONCEPTS.md,V1-CANDIDATES.md}`. Prior riders hold.

**Posture.** Stable track (0.3.1). One additive `AcceptanceCheck::Rules` variant (serde-additive, backward-compatible) — no `PipelineState`/`AcceptanceMarker` schema breakage. Nonce isolation, the dr-gate subprocess boundary, and `validate_acceptance_marker` are unchanged; rules ride the existing evaluate-then-sign path. Rule evaluation is deterministic (regex/glob over file bytes), no provider call. No `git push`. Edits inside `/Users/gdc/deadreckon`. Major decisions → V1-CANDIDATES.

**A rules file, enforced and tamper-evident.**

- `deadreckon-rules.yaml` lists rules: each has an id, a `forbid` or `require` pattern, path globs (+ `exclude`), and a human message.
- Rules evaluate against the run's **touched files** (from provenance) by default — the agent is judged on what it changed — with an opt-in `scope: all` per rule for whole-tree invariants.
- When the file is present, `compiled_acceptance_checks` auto-adds a `Rules` check; an operator can also reference it explicitly in `acceptance.yaml`.
- A failing rule refuses "done" non-terminally (the existing gate-retry loop) with the specific rule id + file:line fed back to the agent as a corrective hint.

**Tamper binds the rules.** The rules file bytes fold into `marker_signature`, and editing/deleting `deadreckon-rules.yaml` mid-run, or weakening a rule the run was subject to, classifies `refuse` — the same protection acceptance.yaml already gets. An agent cannot make the gate green by gutting the rules.

**Friendliness.** Preview the active rules in the run preflight and in `detect`. A rule violation refuses with `try:` naming the rule and the offending file:line. Rule results surface in `verdict` evidence.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `make verify` green → conventional-commit → CHANGELOG line. P11 adds AS-BUILT §13/§35.

**Verification.**

- Every rider depth test present and passing; `make verify` green each commit.
- A run that introduces a forbidden pattern in a touched file fails the gate with the rule id + file:line; fixing it passes.
- Deleting/weakening `deadreckon-rules.yaml` mid-run yields a tamper `refuse`; the rules bytes are in the marker signature. No `git push`. No `PipelineState`/marker schema breakage.

**Stop when** verification passes, AS-BUILT + V1-CANDIDATES + a `Rules (stable)` CHANGELOG section are updated, committed locally.
