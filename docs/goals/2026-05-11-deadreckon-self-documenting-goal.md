GOAL: Make every deadreckon run at `/Users/gdc/deadreckon/` produce a human-readable document set in the **stoa documentation shape**, automatically as the run progresses, so a person can absorb what a long-running task changed without reading `traces.jsonl` or scrolling commit logs. Pattern mined from `/Users/gdc/stoa/docs/{implementation,design}/` + per-subsystem `AS-BUILT-ARCHITECTURE.md`, and the "doc parity" commit discipline visible in `git -C /Users/gdc/stoa log -- docs/implementation/`.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` — substrate.
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-deadreckon-self-documenting-rider.md` — schemas, frontmatter, skill mechanism, depth tests.
- Stoa exemplars: `/Users/gdc/stoa/docs/implementation/2026-05-07-MEETING-AUTO-CAPTURE-AND-TRANSCRIPTION.md` + `/Users/gdc/stoa/stoa-cli/pkg/scribe/AS-BUILT-ARCHITECTURE.md`.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` — invariants hold.

**Posture.** Stays `alpha`. No `PipelineState` schema changes (docs are files). No `git push`. V1 decisions → `docs/V1-CANDIDATES.md`.

**Three artifacts (one optional fourth) under `working/.deadreckon/docs/`.**

- **`RUN-NARRATIVE.md`** — impl-doc analogue. Stoa frontmatter (Date / Status / Run ID / Commit span / Owner / Goal); phase sections; "Updates since"; "Open threads". Citations to trace turn-ids.
- **`RUN-AS-BUILT.md`** — subsystem-AS-BUILT. System overview; component table; flow ASCII when applicable.
- **`RUN-DECISIONS.md`** — one entry per detected decision: alternatives, chosen, why, turn link.
- **`AS-BUILT-DELTA.md`** (conditional, worktree commits to branch) — amendment when source has `AS-BUILT-ARCHITECTURE.md`.

**Generation cadence.**

- **Per turn (no LLM call):** append a narrative chunk (header + tool calls + files + commit SHA in worktree).
- **End of run (one polish call):** prompt is the `run-narrator` skill (`skills/run-narrator/SKILL.md`); user/project overrides via three-tier resolution (printing-press pattern, judgment in markdown). doc-writer provider produces all three docs as JSON. Idempotent; failure non-fatal.
- **On `apply`:** commit body = executive summary + phase list (rider template).

**New verb.**

- `deadreckon doc <run-id> [--kind narrative|as-built|decisions|delta] [--export <path>] [--polish]` — prints (default narrative); `--polish` forces a fresh pass.

**Friendliness.**

- Frontmatter mirrors stoa (`**Date:**`, `**Status:**`, `**Commit span:**`, `**Owner:**`, `**Run ID:**`, `**Goal:**`).
- Each turn entry cross-links `traces.jsonl#turn-N`, `snapshots/turn-N/`, branch SHA.
- **Diff coverage**: every file in the diff appears in `RUN-NARRATIVE.md` (verified at promotion; missing → polish retry).
- Extend/merge updates the parent's narrative with an "Updates since" section (stoa's doc-parity habit).
- `--no-docs` disables LLM polish; templated narrative still writes.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check` green → conventional-commit → CHANGELOG. P11 adds "Self-Documenting Runs" to AS-BUILT, updates §22.

**Verification.**

- Commands green every commit; every rider depth test present and passing.
- Worktree smoke: `run "rename Foo to Bar" --yes` produces `docs/{RUN-NARRATIVE,RUN-AS-BUILT,RUN-DECISIONS}.md` (frontmatter + ≥1 phase + ≥1 citation each); promotion lands them in `library/.../docs/`; `apply <id>` body matches the rider template.
- Diff coverage: 5-file fixture → all 5 named in narrative. Extend smoke: extended narrative has "Updates since" linking parent. `--no-docs`: incremental only, zero doc-provider calls.
- No edits outside `/Users/gdc/deadreckon/`. No `git push`. No schema changes.

**Stop when** verification passes, AS-BUILT updated, CHANGELOG has a "Self-documenting runs (alpha)" section, committed locally.
