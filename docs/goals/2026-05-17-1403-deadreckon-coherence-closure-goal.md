GOAL: Finish the coherence pass so every user-facing surface of `deadreckon` uses the same words, colors, streams, flags, prompts, and next-action grammar across runs, plans, chains, finish/apply/export, providers, done criteria, docs, and JSON/plain output. The refreshed audit is `/Users/gdc/deadreckon/docs/design/USER-FACING-MATRIX.md`; treat it as the source of truth. Headline word: **Coherent**.

**Read first.**

- `/Users/gdc/deadreckon/docs/design/USER-FACING-MATRIX.md` - current matrix and backlog.
- `/Users/gdc/deadreckon/docs/goals/2026-05-17-1403-deadreckon-coherence-closure-rider.md` - exact implementation constraints and tests.
- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - §§17, 18, 26, 30, 32.
- `/Users/gdc/deadreckon/crates/deadreckon/src/{cli.rs,main.rs,ui.rs,prompt.rs}`
- `/Users/gdc/deadreckon/crates/deadreckon-core/src/glossary.rs`

**Posture.** Stays `alpha`. Preserve existing durable schemas unless a rider phase proves a tiny additive field is required. Do not remove aliases casually; make canonical user-facing examples consistent, and keep compatibility aliases hidden or secondary. No `git push`. Edits stay inside `/Users/gdc/deadreckon/`. V1-scale changes go to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**Core idea.**

- One glossary for nouns, verbs, statuses, object kinds, provider roles, done criteria, strategies, and lifecycle actions.
- One command/help table feeding top help, help-all, clap after-help examples, and docs examples.
- One flag policy for `--yes`, `--no-confirm`, `--all`, `--all-scopes`, `--branch`, `--max-spend`, `--strategy`, `--plain`, `--quiet`, `--json`, `--no-hints`, and provider-role flags.
- One style and palette helper for headings, ids, commands, status tones, warnings, errors, hints, try-lines, progress, and TUI colors.
- One prompt/preflight builder, especially for orchestrate mode, child count, provider roles, repair, source mode, caps, and done criteria.
- One lifecycle summary builder so a plan feels like a run: plan id primary, child/result run ids secondary, `finish <id>` first, direct apply/export only after that.

**Keep the visual fun.** Preserve the cyan `deadreckoning` banner, `* ^ . -` course strip, magenta ids, spend gauge gradient, and step glyphs `○ ● ◐ ✗ ↷ ◉ ↶`. Coherence is not blandness.

**User experience.**

- `deadreckon --help`, `help-all`, and command help never disagree.
- A user sees `status`, `finish`, `apply`, `export`, `cleanup`, `def-done`, `plan`, and `child` used consistently, with aliases secondary.
- `run`, `extend`, `resume`, `orchestrate`, and `chain` summaries share the same shape.
- `orchestrate` interactive preflight helps the user choose review vs full-plan, child count, planner/coder/reviewer/child providers, repair, caps, source mode, and done criteria before execution.
- `list`, `status`, `show`, `attach`, `finish`, `kill`, and `cleanup` make run ids, plan ids, and child refs easy to map.
- JSON/plain/quiet/hints obey a documented policy with tests.

**Phases.** Eleven phases in the rider. Each phase starts with focused depth tests or snapshots, then implementation, then focused green tests. Milestone boundaries run `cargo build --release`, `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, and `cargo fmt --check` when practical. Update CHANGELOG and AS-BUILT at the end.

**Verification.** Matrix findings H/F/S/L/O/P/J are fixed or explicitly deferred. Help/output snapshots prove canonical words, stable streams, and aligned top/help-all/command help. JSON tests prove no ANSI/hints leak. TUI render tests preserve colors, glyphs, and standard footers. Docs examples use canonical commands.

**Stop when** the coherence closure is implemented, focused and workspace verification pass, AS-BUILT/CHANGELOG/docs are updated, V1 deferrals are logged, and the work is committed locally.
