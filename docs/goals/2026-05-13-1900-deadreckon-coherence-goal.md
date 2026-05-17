GOAL: Make every user-facing surface of `deadreckon` say the same word, colour it the same, print on the same stream, respond to the same flag the same way. Keep the visual fun: the `deadreckoning` cyan banner, the `* ^ . -` course strip, magenta IDs, the spend gauge gradient, the step glyphs `○ ● ◐ ✗ ↷ ◉ ↶`. The audit at `docs/design/USER-FACING-MATRIX.md` lists 108 inconsistencies; unpushed orchestration commits add five verbs (`orchestrate`, `plan`, `fork`, `merge`, `history`) and a third TUI (plan attach) that need the same model. One glossary, one style helper, one prompt builder, one kv-block, one palette, one truth for `--force`/`--all`/`--branch`/`--max-spend`/`--strategy`. Headline: **Coherent**.

**Read first.**

- `/Users/gdc/deadreckon/docs/design/USER-FACING-MATRIX.md` - the spec.
- `/Users/gdc/deadreckon/docs/goals/2026-05-13-1900-deadreckon-coherence-rider.md` - phases, depth tests, glossary, refusal pairs, orchestration coverage.
- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - gets section 26 in P11.
- `/Users/gdc/impeccable/STYLE.md` - editorial brief.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/`. Invariants hold.

**Posture.** Stays `alpha`. No `PipelineState` schema changes. No `RunStatus`/`ChainStatus`/`PlanTaskStatus` variant renames; the displayed string changes via one `status_label()`. No `git push`. Larger renames go to `docs/V1-CANDIDATES.md`.

**One glossary, one helper set, one builder.**

- **Glossary** in `crates/deadreckon-core/src/glossary.rs`: status words (`running` not `executing`), nouns (`run`/`chain`/`plan`/`child`; never `task`), verbs. Plan-child status uses run-status labels.
- **Style** in `crates/deadreckon/src/ui.rs`: one `Tone` enum, one `write(stream, tone, text)`. Retires `ui_error_stdout` and the six raw-ANSI sites at `main.rs:204, 4805, 6626-6627, 7082, 7218`. Course strip and `with_cli_wait_status` spinner stay; only colour calls migrate.
- **`print_kv_block`** replaces five drifting layouts and adopts `print_plan_summary` + `print_plan_created`.
- **`prompt::confirm`** is the only Y/n + y/N builder. Fixes `main.rs:5936` and `8891`.

**Flag truth.** `--force` splits into `--escalate` (kill), `--overwrite` (dest), `--anyway` (override). `--all` stays cross-project; cleanup/chain take `--all-scopes`; status takes `--global`. `--budget-cap` becomes `--max-spend` on `doc`. `--branch` splits: `--branch-name` on `run`, `--into` on `apply`/`finish`. `--strategy` collides three ways today; rename apply to `--git-strategy` and rename chain branch-policy value `merge` to `linear-merge`. Aliases kept one alpha.

**Phases.** Eleven (P1-P11). Each: depth test first, implement, `cargo nextest + fmt + clippy` green, conventional commit, CHANGELOG. P11 adds section 26 to AS-BUILT.

**Verification.**

- Every status string in `main.rs` flows through `status_label()`; zero direct ANSI codes outside `ui.rs` (grep tests).
- `deadreckoning` banner, course strip, step glyphs, gauge gradient render byte-for-byte as today (goldens).
- `status latest` on completed/failed/running runs shares one kv layout (golden).
- `chain show` and `chain attach` snapshot share one header at six-decimal precision.
- `attach <plan-id> --plain` and `merge <plan-id>` say `running` for an in-flight child, matching `status <run-id>`.
- `attach <id>` prints `attaching to run|chain|plan <prefix>` to stderr before TUI clears.
- `kill <run-id>`, `chain kill`, `kill <plan-id>` all route through `print_kill_banner`; plan output adds `(N processes signalled)`.
- `show --why-failed` and `chain show --why-failed` share one `render_why_failed` layout.
- `doctor` + `detect` + `providers list` use the same `kind=` vocabulary.
- `history grep` honours `--all`, `--scope`, `--plan` the same way as `list` and `cleanup`.

**Stop when** verification passes, AS-BUILT and CHANGELOG describe "Coherence pass (alpha)", deferred renames and theming are in `docs/V1-CANDIDATES.md`, work is committed locally.
