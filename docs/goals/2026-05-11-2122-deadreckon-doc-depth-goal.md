GOAL: Make every deadreckon run produce documentation as substantive as the stoa exemplars at `/Users/gdc/stoa/docs/{design,implementation}/` and `/Users/gdc/stoa/stoa-cli/pkg/scribe/AS-BUILT-ARCHITECTURE.md`. Today's run docs (e.g. `/Users/gdc/test-deadreckon/build-a-full-ms-paint-ty/docs/`) are 60-line templated stubs with robotic summaries, generic component tables, outcomes truncated mid-sentence at 200 chars, and the polish call never runs (`Doc-writer: templated only`). Headline word: **Depth**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` (especially §25).
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-1525-deadreckon-self-documenting-rider.md` — predecessor; invariants hold.
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-2122-deadreckon-doc-depth-rider.md` — schemas, prompts, depth tests.
- Stoa target shape: `/Users/gdc/stoa/docs/implementation/2026-05-07-MEETING-AUTO-CAPTURE-AND-TRANSCRIPTION.md`, `/Users/gdc/stoa/stoa-cli/pkg/scribe/AS-BUILT-ARCHITECTURE.md`.
- Current shallow output: `/Users/gdc/test-deadreckon/build-a-full-ms-paint-ty/docs/`.
- `/Users/gdc/deadreckon/skills/run-narrator/SKILL.md` — current shallow skill.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` — invariants hold.

**Posture.** Stays `alpha`. No `PipelineState` schema changes. No `git push`. Edits inside `/Users/gdc/deadreckon/`. V1 decisions → `docs/V1-CANDIDATES.md`.

**Code changes (full prescriptions in the rider).**

- Auto-resolve `doc_provider` to the active subscription CLI provider when none is configured (no more `templated only`).
- Capture full provider output per turn (50 KB cap, not 200 chars); capture per-file diff samples + bash stdout/stderr.
- Fix narrative `# heading` truncation (use full goal).
- Path-driven component-table inference; process-topology ASCII when ≥3 top-level directories changed.

**Skill changes.**

- `run-narrator` splits into 4 sub-skills (`narrator-overview`, `narrator-phases`, `narrator-as-built`, `narrator-decisions`); same three-tier resolution; 16 K output tokens per sub-call.
- Each sub-skill asks for stoa-shape material: components with file:line, topology ASCII, wire protocols, "load-bearing" / "seams" sentences, "Reading order" preamble, supersession markers.
- New placeholders: `{{ diff_samples }}`, `{{ tool_stdout }}`, `{{ source_layout }}`, `{{ parent_narrative }}`.

**Friendliness as a verifiable contract.**

- Auto-detect doc_provider; refuse only when nothing usable with `try:`.
- Preview doc-call cost + sub-skill list before each polish.
- Refuse with `try:` on every error footer.
- `deadreckon doc <id> --polish --force` re-runs against the new prompts.
- Lifecycle hints after every action.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check` green → conventional-commit → CHANGELOG. P11 updates AS-BUILT §25 + §22.

**Verification.**

- Commands green every commit; every rider depth test present and passing.
- Re-polish smoke: `deadreckon doc <id> --polish --force` against a 1-turn fixture yields `RUN-NARRATIVE.md` ≥ 250 lines, components table with ≥1 file:line per row, ≥1 prose paragraph per phase, no 200-char truncation.
- Auto-provider smoke: with `cli:codex` in `$PATH` and no `doc_provider` set, polish runs (frontmatter shows `Doc-writer: cli:codex`).
- No edits outside `/Users/gdc/deadreckon/`. No `git push`. No `PipelineState` schema changes.

**Stop when** verification passes, AS-BUILT updated, CHANGELOG has a "Doc depth (alpha)" section, committed locally.
