GOAL: Unify provider and done-criteria setup so `init`, `config provider`, `run`, `orchestrate`, doc polish, and `def-done` stop carrying separate setup logic. Today the pieces work, but each surface resolves providers, prompts, models, doc providers, and acceptance specs in its own local way. Land the next V1 polish slice: one reusable setup substrate for provider roles and done criteria, with shared previews, refusal wording, and docs/help sources. Headline word: **Prepared**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - provider model, acceptance gate, config, user-facing style.
- `/Users/gdc/deadreckon/docs/design/USER-FACING-MATRIX.md` - P2/P5 plus L9/L10/O2 deferrals.
- `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` - provider and done-criteria setup unification deferral.
- `/Users/gdc/deadreckon/docs/goals/2026-05-24-1426-deadreckon-provider-done-setup-rider.md` - implementation contract.
- Current code: `/Users/gdc/deadreckon/crates/deadreckon/src/{main.rs,cli.rs,prompt.rs}` and provider registry/config crates.
- Prior riders: coherence closure, doc-depth, provider registry, orchestration event bus, implementation notes.

**Posture.** Stays `alpha`. No `PipelineState` schema changes. No new top-level verb; keep `def-done` canonical and `acceptance` hidden compatibility. Avoid new durable config keys unless the rider proves they are tiny and backward-compatible. No `git push`. Edits stay inside `/Users/gdc/deadreckon/`. V1-scale decisions go to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**Core deliverables.**

- A shared provider setup resolver for primary run, doc polish, planner, child, coder, reviewer, repair, and config/default-provider surfaces. It reports route, model, source, credential/install state, warnings, and `try:` lines.
- `init`, `config provider`, `run`, `orchestrate`, and `doc --polish` use the same resolver/preview vocabulary instead of local provider prompt branches.
- A shared done-criteria resolver for explicit `--acceptance`, project `.deadreckon/acceptance.yaml`/`.md`, generated criteria, and default dr-gate behavior.
- `def-done` and hidden `acceptance` share docs/help text and error hints; normal user-facing surfaces say "done criteria", while files and technical rows may still say `acceptance.yaml` or gate.
- Non-git/source-mode preflight, provider-role selection, doc-provider selection, and done-criteria setup produce consistent preview rows and non-interactive refusals.
- Existing provider routing, doc polish auto-subscription behavior, signed acceptance gate invariants, and orchestration role semantics remain behavior-compatible.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implement -> focused verification -> conventional local commit -> CHANGELOG. P11 updates AS-BUILT, USER-FACING-MATRIX, and V1-CANDIDATES.

**Verification.**

- Focused matrix green: setup/coherence tests, config/provider tests, doc polish tests, def-done/acceptance tests, orchestration preflight tests, fmt, and clippy for touched crates.
- Smokes: `deadreckon init --no-confirm` and `deadreckon config provider` agree on the selected provider/default model wording; `deadreckon run --preview` and `deadreckon orchestrate --preview` show the same done-criteria source for the same workspace; doc-polish confirmation text uses the same provider-source labels as setup.
- Do not run `make verify`, release builds, smoke, stress, or full-workspace tests by default.
- No edits outside `/Users/gdc/deadreckon/`. No `git push`. No `PipelineState` schema changes.

**Stop when** verification passes, AS-BUILT/CHANGELOG/USER-FACING-MATRIX/V1-CANDIDATES describe "Provider and done-criteria setup unification (alpha)", and the work is committed locally.
