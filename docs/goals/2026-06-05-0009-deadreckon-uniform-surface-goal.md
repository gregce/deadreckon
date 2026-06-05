GOAL: Make DeadReckon's non-TUI terminal output and one-shot prompts uniform, correct, and friendly — one styling layer, one width function, shared kv/table primitives, and a hardened prompt engine. Today the same thing renders ~four ways (two divergent `Tone` enums, per-call-site alignment, five "next step" markers), an alignment bug short-pads colored columns, `--no-hints` is silently ignored on some surfaces, and number menus break past nine choices. Land a follow-up slice named Uniform Surface. The attach TUI is deliberately out of scope (its own goal, `2026-06-05-0010-deadreckon-attach-tui-uniformity-goal.md`).

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-06-05-0009-deadreckon-uniform-surface-rider.md` - phases, the unify rules, depth tests, exact citations.
- `/Users/gdc/deadreckon/crates/deadreckon/src/ui.rs` and `ui_card.rs` - the two divergent Tone enums, width helpers, `pad_visible`.
- `/Users/gdc/deadreckon/crates/deadreckon/src/prompt.rs` - the one-shot menu/select/confirm engine to harden.
- `/Users/gdc/deadreckon/crates/deadreckon/src/{verdict_surface,proof_block}.rs` and `commands/{inspection,providers,plan,campaign,status}` paths - the alignment/hint sites.
- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`, `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`. Prior riders' invariants hold.

**Posture.** Production-release track; presentation refactor, not a feature. No runtime-state schema changes. No CLI verb additions beyond an interactive goal prompt for `deadreckon start`. No `git push`. Edits inside `/Users/gdc/deadreckon`. Byte-exact render tests are the contract — update goldens deliberately, never loosen them.

**Library decisions (verified, do not re-litigate).** Adopt `unicode-width` as a direct dep (ratatui already pulls it; zero tree weight) for one `display_width()`. Adopt `textwrap` (MIT) for prose wrapping, or one hand-rolled wrapper on `display_width`. Keep styling, tables, prompts, and spinners hand-rolled and unified — do NOT add console/dialoguer/inquire/comfy-table/indicatif.

**Uniformity contract.**

- One `Tone` enum, one tone->ANSI table and one tone->ratatui::Color table derived from it; no silent-dim fallback for unknown status.
- One `display_width()` (strip ANSI, then unicode-width) behind every width/pad/truncate site; no `{:<N}` over already-styled strings.
- Two shared columnar primitives: `kv_block` (auto-sized lowercase `key: value`) and `columns` (lowercase header, display-width padding, terminal fit, ellipsis).
- One next-step marker; bare `println!("cancelled")`/raw dumps become a VerdictSurface with a Recommended step.
- Prompts: multi-digit number entry in menu AND line mode, identical rendering, Esc always cancels, out-of-range feedback, validated `ask_number(range)` that re-prompts instead of hard-erroring.

**Phases.** Eleven in the rider. Each: write the named depth test(s) first and watch them fail; make the narrow change; `cargo fmt --check`, `cargo clippy --workspace --all-targets`, and focused `cargo test` green; conventional-commit; one-line CHANGELOG. P11 updates AS-BUILT, V1-CANDIDATES, CHANGELOG.

**Verification.**

- Every rider depth test present and passing; `cargo test -p deadreckon` green.
- A colored column aligns identically with color on and off (the `{:<N}`-on-ANSI bug is gone), asserted by a guard test.
- `--no-hints` / `DEADRECKON_HINTS=0` suppress hints on status, campaign, inspection, doc, and chain surfaces.
- A >9-choice menu is reachable by number in both menu and line mode; a bad count re-prompts rather than exiting.
- `cargo fmt --check` and `git diff --check` clean.

**Stop when** output renders through one Tone, one width function, and the shared kv/table primitives; prompts are hardened per the contract; the hint-polarity and alignment bugs are fixed; verification passes; AS-BUILT/V1-CANDIDATES/CHANGELOG updated; committed locally.
