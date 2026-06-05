GOAL: Make the attach TUI behave identically across every surface you can attach to — run, plan, campaign, and chain, plus the completion overlay and narrative view. Today each surface re-implements its own key dispatch, selection glyph, footer, scroll readout, and confirm behavior, so the controls change with what you attached to: paging keys work in run-attach but not plan/campaign, the highlight is `*` here and `>` there, destructive Apply/Abandon fire with no confirmation, and chain step glyphs have no ASCII fallback. Unify the chrome around the run-panel reference while keeping each mode's content panel. Land a follow-up slice named Attach TUI Uniformity.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-06-05-0010-deadreckon-attach-tui-uniformity-rider.md` - phases, the four handlers, depth tests, citations.
- `/Users/gdc/deadreckon/crates/deadreckon/src/tui/attach_state.rs` - run-panel `handle_key` (the reference).
- `/Users/gdc/deadreckon/crates/deadreckon/src/tui/render.rs` - chain `handle_key`, glyphs, footers, scroll indicator.
- `/Users/gdc/deadreckon/crates/deadreckon/src/commands/attach.rs`, `attach_runtime.rs` - plan loop, `handle_campaign_key`, completion overlay.
- `/Users/gdc/deadreckon/docs/goals/2026-06-05-0009-deadreckon-uniform-surface-rider.md` - sibling slice; reuse its primitives.
- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`, `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`. Prior riders' invariants hold.

**Posture.** Production-release track; TUI presentation refactor, not new capability. No runtime-state schema changes. No `git push`. Edits inside `/Users/gdc/deadreckon`. Prefer landing AFTER Uniform Surface so the shared `Tone`/`display_width`/`kv_block` exist; if landing first, stub minimally and note the dependency rather than duplicating. Byte-exact TUI render tests are the contract; update goldens deliberately.

**Principle: uniform chrome, mode-specific content.** Unify everything around the panel; each mode keeps its own body (run turn stream, plan child-ref tree, campaign feed, chain step graph, narrative projection). The detailed contract — one key dispatcher with a per-mode hook, one glyph, one footer builder, one scroll indicator, confirm-before-destructive, uniform exit/return, friendly empty states, Windows ASCII glyph fallbacks — is in the rider.

**Phases.** Eleven in the rider. Each: write the named depth test(s) first and watch them fail; make the narrow change; `cargo fmt --check`, `cargo clippy --workspace --all-targets`, and focused `cargo test` green; conventional-commit; one-line CHANGELOG. P11 updates AS-BUILT, V1-CANDIDATES, CHANGELOG.

**Verification.**

- Every rider depth test present and passing; `cargo test -p deadreckon` green.
- PgUp/PgDn/Home/End/g/G and the selection glyph and footer shape are identical across run, plan, campaign, and chain (asserted by golden/characterization tests).
- Apply and Abandon require a confirm keystroke; a single mistyped key cannot fire them.
- Every list panel shows a scroll-position indicator; no empty state prints an internal filename.
- `cargo fmt --check` and `git diff --check` clean.

**Stop when** all attach surfaces share one dispatcher, glyph, footer, and scroll indicator; destructive actions confirm; navigation keys are at parity with the run panel; verification passes; AS-BUILT/V1-CANDIDATES/CHANGELOG updated; committed locally.
