GOAL: Land a local-first self-improvement loop for DeadReckon that turns prior run evidence into proposals, runs safe candidates against DeadReckon itself, and can auto-open a GitHub PR only after an evidence gate passes. Existing runstate, traces, provenance, gates, plan events, flight logs, and docs become a cautious learning loop rather than model training. Headline word: **Improving**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - runstate, gates, orchestration, import, flight recorder.
- `/Users/gdc/deadreckon/docs/goals/2026-05-26-1325-deadreckon-self-improvement-loop-rider.md` - schemas, commands, evidence gate, PR criteria.
- `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` - deferred import replay, analytics, provider routing, flight semantics.
- `/Users/gdc/deadreckon/docs/goals/2026-05-25-2238-deadreckon-provider-flight-recorder-goal.md` - flight/checkpoint substrate.
- Prior provider, setup, orchestration, and event bus riders in `/Users/gdc/deadreckon/docs/goals/` - invariants hold.

**Posture.** Stays `alpha`. No `PipelineState` schema changes. Learning state is file-backed under `DEADRECKON_HOME/learning/` and candidate run roots. No raw provider logs, secrets, credentials, or home paths in shared bundles. Live PR opening is an explicit product capability after evidence passes; this goal verifies it with dry-run/fake GitHub adapters only. No live `git push` while executing the goal. Edits stay inside `/Users/gdc/deadreckon/`. V1-scale learning, sync, or training goes to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**New verbs.**

- `deadreckon learn index [--scope <scope>|--all]` - index completed runs into redacted episodes, signals, and outcome labels.
- `deadreckon learn report [--json]` - summarize repeated friction, failure modes, provider/mode patterns, and candidate opportunities.
- `deadreckon learn propose [--from-local|--bundle <path>] [--limit N]` - write improvement proposals tied to concrete evidence.
- `deadreckon improve self <proposal-id|goal-file> --preview|--yes [--open-pr|--pr-dry-run]` - run a candidate against DeadReckon in an isolated worktree, verify it, archive evidence, and optionally prepare/open a PR.

**Evidence gate for PR opening.**

- Requires opt-in, clean base, isolated worktree, non-weak done criteria, accepted candidate run, focused verification green, no redaction/secrets findings, and evidence linking stimulus -> proposal -> diff -> verification -> rollback.
- Refuses auto-PR for high-risk changes by default: sandbox, gate/signature, provider credential/config, release/CI, acceptance-policy, or privacy/redaction weakening.
- PR body includes run ids, proposal id, evidence score, changed files, verification, risk, and rollback. `--pr-dry-run` produces the same body without network or push.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implement -> focused verification green -> conventional local commit -> CHANGELOG. P11 updates AS-BUILT, V1-CANDIDATES, and CLI docs.

**Verification.**

- Focused tests only: learning/index/proposal/self-improve/PR-gate tests, CLI snapshots for touched commands, fmt, clippy for touched crates, and targeted cargo tests.
- Smokes: synthetic runs index into proposals; self-improve dry-run archives evidence; fake GitHub PR open succeeds only after the evidence gate passes.
- Do not run `make verify`, release builds, stress tests, broad smokes, or full-workspace tests by default.

**Stop when** focused verification passes, `--pr-dry-run` proves the PR path without network side effects, docs record shipped limits honestly, V1 deferrals are updated, and the work is committed locally.
