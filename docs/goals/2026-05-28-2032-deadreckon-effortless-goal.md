GOAL: Evaluate the entire deadreckon surface for friction, then land the highest-leverage ease-of-use improvements so an already-strong tool feels inevitable. The engine is powerful but the *first ten seconds*, the *return after walking away*, and the *vocabulary* still ask too much. Make the friendly path the default everywhere — **without** touching core mechanism (gate, sandbox, promotion, providers, campaign engine stay as built). Friendliness becomes a contract the tests enforce. Headline word: **Effortless**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` — §17 CLI, §18 attach, §30 plans, §36 campaign.
- `/Users/gdc/deadreckon/docs/goals/2026-05-28-2032-deadreckon-effortless-rider.md` — full contract.
- `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs` — exit cards, status, finish, hints.
- `/Users/gdc/deadreckon/crates/deadreckon/src/ui_card.rs`, `.../src/setup.rs` (`auto_subscription_cli_provider`).
- `README.md`, `HOWTO.md`; prior riders in `docs/goals/` — invariants hold.

**Posture.** Production-release track; presentation/UX plus one advisory provider call (the goal classifier reuses provider routing, never changes it). Files-not-fields: no `PipelineState`/`Plan`/`Campaign`/provider schema changes. Reuse `ui_card`/`ui`/`glossary`, not a new framework. No `git push`. Edits inside `/Users/gdc/deadreckon/`. Big bets (palettes, localization, template engine, notifier daemon) → `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**Friendliness as a verifiable contract (each ships with a depth test).** Auto-detect don't ask; preview before any state change; refuse with a `try:` line; one-command rollback; one verdict + ONE primary action; lifecycle hints.

**What lands (detail in rider).**

- **P1 evaluation:** a friction audit codifying the contract as a curated checklist + targeted tests; it drives and confirms the later phases.
- **`deadreckon try`:** zero-arg keyless smoke run that signs the gate and prints a "here's your proof" block (gate SIGNED, `RUN-NARRATIVE.md` path, one provenance line) — the differentiator felt in ~10s, no credentials.
- **Self-bootstrapping `start`:** adopt the one detected subscription CLI inline instead of refusing with "run `deadreckon init`".
- **One verdict + ONE primary action** on the exit card, `status`, `finish` (paused/failed variants), replacing the 3-4 equal-weight `try:` hints.
- **Consistency check:** honest spend + per-check gate verdicts (from tamper-evident) render on *every* surface; fix stragglers.
- **Opt-in notifications** on accepted / paused-at-cap / failed (native + `command`/webhook), config-driven; a daemon is V1.
- **Provider-backed goal routing:** `start` asks the planner provider whether a goal is one run, one orchestration, or a **campaign** (+count +rationale), shown as a suggestion; deterministic fallback when no provider. Campaign `--n` optional, recommendation-seeded, editable.
- **Vocabulary:** the guarantee is a **"verified run"** (verified by `dr-gate`); done-criteria under one umbrella; "5 verbs to live by, ~12 via `help-all`".
- **Error-footer coverage:** every refusal carries a `try:` line.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implement -> focused tests green -> conventional local commit -> CHANGELOG. P11 updates AS-BUILT and logs deferrals.

**Verification.**

- Every rider depth test present and passing; `cargo fmt --check`; `git diff --check`.
- Smoke: `deadreckon try` signs a gate and prints the proof block, no keys.
- Smoke: an accepted run fires the configured notification (command channel).
- No edits outside the repo; no `git push`; no schema changes.

**Stop when** the audit is recorded, the friendliness contract passes for every audited verb, `try`/self-bootstrap/one-verdict/notifications/vocabulary/campaign-friendliness/error-footers are in and tested, AS-BUILT and CHANGELOG record the pass, deferrals are in V1-CANDIDATES, and the work is committed locally.
