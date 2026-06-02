GOAL: Converge every DeadReckon failure and completion surface to one verdict, one recommended command, and one explanation panel. Today terminal outcomes are split across status lines, `try:` footers, lifecycle hints, `--why-failed` renderers, repair commands, JSON `next_actions`, and ad hoc command summaries. Land a production-release UX slice named Verdict Surface: when a command finishes, fails, blocks, pauses, or previews a state change, the human sees one decisive outcome, one primary next command, and one compact explanation panel with the evidence behind that decision.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-06-01-1417-deadreckon-verdict-surface-rider.md` - phases, tests, matrix, stop rules.
- `/Users/gdc/deadreckon/docs/FRIENDLINESS-AUDIT.md` - current failures for "One verdict + ONE primary action".
- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - production-release posture and shipped vs V1 boundaries.
- `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` - larger output-layout work stays V1.
- `/Users/gdc/deadreckon/crates/deadreckon/src/ui_card.rs` and `/Users/gdc/deadreckon/crates/deadreckon/src/cards/exit_summary.rs` - existing card and exit summary primitives to reuse.
- `/Users/gdc/deadreckon/crates/deadreckon/src/friendliness_contract.rs` - contract clause that this goal burns down.

**Posture.** Production-release UX work for 0.1.0 readiness. No durable state schema changes. No command removals or broad renames. No `git push`. Edits stay inside `/Users/gdc/deadreckon`. Do not build the full V1 output-layout facade, template engine, localization, rich merge UI, or AST/semantic merge engine; log that pressure in `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**Contract.**

- Every failure/completion surface must render exactly one verdict label: completed, verified, failed, blocked, killed, paused, preview, or no-op.
- Every such surface must render exactly one recommended command. Secondary commands may exist, but they must be visually and structurally subordinate.
- Every such surface must render one explanation panel that answers: what happened, why this is the verdict, and which evidence or state path supports it.
- JSON output remains backward-compatible and additive: keep existing fields, but add a single `verdict`/`primary_action` shape where appropriate.
- `--quiet`, `--json`, `--plain`, and `--no-hints` keep their semantics. Do not make quiet success noisy, but do keep error output decisive.

**Scope.** Normalize terminal surfaces for `run`, lifecycle verbs, `start` previews, `orchestrate`/`plan`, `fork`, `merge`, `campaign`, `campaign repair`, `chain`, recovery verbs, and setup/diagnostic commands that print terminal summaries or recovery hints. Read-only listings need no decorative cards unless they already present completion/failure output, but any recovery footer must obey the one-primary-action rule.

**Phases.** Eleven phases in the rider. Each: write depth tests first; implement the smallest shared primitive that serves current callers; align plain, card, and JSON output; run focused tests plus `cargo fmt --check` and `git diff --check`; commit locally when green. P11 updates AS-BUILT, FRIENDLINESS-AUDIT, V1-CANDIDATES if needed, and CHANGELOG.

**Verification.**

- `cargo test -p deadreckon verdict_surface`
- `cargo test -p deadreckon friendliness`
- Focused command tests for run, plan/orchestrate, campaign, chain, and recovery verbs are present and passing.
- `cargo fmt --check` and `git diff --check` are green.
- FRIENDLINESS-AUDIT shows the one-primary-action failures burned down or reclassified outside terminal-outcome scope.

**Stop when** every in-scope failure and completion surface has one verdict, one recommended command, one explanation panel, docs are updated, verification is green, and the work is committed locally.
