GOAL: Take the front door on a shakedown cruise — one reference resolver, so every verb agrees about what an id is. Today the five commands the README calls "the whole tool" contradict each other: `deadreckon status` refuses with "try `deadreckon list`", `list` prints plan ids and recommends `status latest`, and `status <that id>` answers "not found: run 0c11f68e" — a closed loop between the two most-used orientation verbs, reachable in thirty seconds on a clean checkout. The cause: every verb hand-rolls its own resolution cascade, and no two cover the same kinds in the same order. `show` misses chains, `kill` misses plan children, `status` and `verdict` see runs only, and `latest` means "newest in this scope" to `status` but "newest across all scopes" to `verdict`. 58 call sites, 14 files. This slice lands one `resolve_ref`, one meaning for `latest`, kind-aware refusals that name the verb which accepts the id, a `list` that shows one row per plan, and a cross-verb journey test pinning the invariant a per-verb audit cannot see. Land this slice named Shakedown.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-07-24-1333-deadreckon-shakedown-rider.md` — resolver types, probe order, refusal table, `latest` rule, eleven phases, depth tests.
- `src/main.rs` — `load_cli_run` (:9059), `latest_run` (:11012), `show_command` (:10606), `kill_command` (:9208), `resolve_plan_id` (:4335), `resolve_plan_child_ref` (:6172).
- `commands/verdict.rs` (:57, :77 — the second `latest`); `chain/mod.rs` (:3216); `campaign.rs` (:2227); `inspection.rs` (:4).
- `docs/FRIENDLINESS-AUDIT.md` — scores `status` and `verdict` **pass** on "Refuse with try:", citing the exact dead-end lines. Per-verb audit, between-verb defect.
- `docs/MAP-OF-DEADRECKON.md`. Prior riders hold; Shakedown takes AS-BUILT §56 (§53/§54 hold for Capstan/Drydock).

**Posture.** Stable track. No `PipelineState` schema changes and no on-disk changes — this slice moves resolution logic, not state. Every rewired verb keeps its success output byte-for-byte; only refusals and `list` rendering change. No new config keys, no new crates. No `git push`. Edits inside `/Users/gdc/deadreckon`. Decisions → `docs/V1-CANDIDATES.md`.

**One resolver.**

- `resolve_ref(paths, RefQuery) -> Result<ResolvedRef>` — one probe order, one prefix rule, one ambiguity refusal, for every id-taking verb.
- `ResolvedRef` — `Run` | `PlanChild` | `Plan` | `Chain` | `Campaign`. The caller declares which kinds it accepts.
- **`latest` means one thing:** newest accepted-kind item in scope by `updated_at`; `--all` widens to every scope.

**Refusals that go somewhere.**

- A verb that cannot handle a kind names the verb that can: `status 0c11f68e` → "0c11f68e is a plan, not a run" + `try: deadreckon show 0c11f68e`.
- **Invariant:** no refusal for an id `list` printed may point back at `list`.
- Kind is resolved, never prompted. At most three secondary actions (`doctor` prints ten today).
- `list` shows one row per plan, children folded, prompt scaffolding stripped from goal text.

**Phases.** Eleven (P1–P11) in the rider. Each: named depth tests first, watched red → implement → `make verify` green → conventional commit → CHANGELOG line naming the SHA. The journey test is parameterized per verb and grows one verb per rewire phase, so no phase leaves a red test behind.

**Verification.**

- `make verify` green on every commit; every rider depth test present and passing.
- Journey: for every id `list`/`list --all` print, each of `status`, `show`, `verdict`, `attach`, `finish`, `kill` either succeeds or refuses with a `try:` that itself accepts that id.
- Reproduction closed: `status` → `list` → `status <id>` ends in an answer, not the loop above.
- `show`/`attach` goldens unchanged; `public_surface` baseline updated on purpose.

**Stop when** verification passes, AS-BUILT has §56, CHANGELOG has a "Shakedown (stable)" section naming each phase SHA, and the work is committed.
