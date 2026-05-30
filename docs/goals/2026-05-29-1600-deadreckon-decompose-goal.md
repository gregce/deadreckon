GOAL: Decompose the 40k-line binary `main.rs` behind a stable facade. The `deadreckon` binary is one 40,597-line `main.rs`: ~11k lines of flat command handlers/helpers plus ~29k lines of inline `#[cfg(test)]` modules in one global namespace with no `commands/` structure — unnavigable, every change a triple-edit. Because the binary crate is **not** in the public-surface baseline (only the four library crates are), relocating it into private modules is zero-surface-risk — but only once a CLI-output characterization net exists, since current tests assert state and files, not stdout/stderr. This lands that net first, lifts the test modules out, then carves command families and the TUI render layer into private `commands/` and `tui/` modules behind a thin `main_inner` dispatcher. Headline word: **Decompose**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` — substrate; latest section §37.
- `/Users/gdc/deadreckon/docs/goals/2026-05-29-1600-deadreckon-decompose-rider.md` — full contract, eleven phases.
- `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs` (40,597 lines), `.../src/cli.rs` — the targets.
- `/Users/gdc/deadreckon/tests/.public-surface-baseline` — surface that must not move.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` — invariants hold.

**Posture.** Behavior-preserving refactor only: every command's stdout/stderr, exit codes, and side effects stay byte-identical. `make verify` (fmt --check + clippy -D warnings + public-surface + `cargo test --workspace` + release build) is green at **every** commit, including each move. The baseline cannot move; the binary crate is not in it. No schema or behavior changes. Moves are mechanical: relocate code, widen visibility only to `pub(crate)` (never `pub`, re-exporting nothing new), never edit logic in a move commit. No `git push`. Edits inside `/Users/gdc/deadreckon/`. Needless-churn rejected; major decisions and deferrals → `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**Workstreams.** The headline carve is into `src/commands/{chain/,orchestrate.rs,plan.rs,campaign.rs,attach.rs,run.rs,init.rs,acceptance.rs,merge.rs}` behind a thin `main_inner` match. Then `src/tui/` for the pure render layer, then cheap cleanups: unify the three `compose_*` merge loops and two command-exists impls, dedupe `is_retryable_io_kind`, prune unused `tracing`/`chrono`, delete dead code, harden the docs.rs regex init. **Non-goals (rejected churn):** core/providers `pub mod`→`mod`; encapsulating Chain/Plan fields; accessor/`#[non_exhaustive]` insurance; splitting the `cli.rs` Commands enum; a CommandHandler trait; a fat `pub run()` facade; `#[source]`/sysexits changes; reshuffling test files.

**Phases.** Eleven (P1–P11) in the rider, characterization-tests-first (RED before refactor): P1 net, P2 shared test helpers, P3 lift inline test modules, P4 commands/ + chain family, P5 remaining families, P6 tui/, P7 merge helper, P8 command-exists, P9 `is_retryable_io_kind` (the single justified re-baseline, isolated at the tip), P10 deps/dead-code/regex/alloc nits, P11 adds **AS-BUILT §38** and logs deferrals. Each: test first → refactor → `make verify` green → commit → CHANGELOG.

**Verification.**

- `make verify` green at every commit; lints satisfied; `cargo test --workspace` reports the same test count and names after each move.
- Characterization snapshots from P1 pass **before and after** each split, zero diff.
- Baseline unchanged, except the one P9 path re-baselined with written justification.
- Equivalence smoke: a sampled command (e.g. `plan --draft`) gives identical stdout/stderr pre/post; an off-TTY attach frame matches golden.

**Stop when** verification passes, `main.rs` is decomposed behind a stable `main_inner`/`commands/`/`tui/` facade with the source well under threshold, the characterization net is in place, AS-BUILT §38 and CHANGELOG record the layout, deferrals are in V1-CANDIDATES, and work is committed locally.

