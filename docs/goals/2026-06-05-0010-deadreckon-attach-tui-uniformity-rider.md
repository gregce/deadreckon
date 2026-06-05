# DeadReckon — Attach TUI Uniformity Rider (one dispatcher, glyph, footer, scroll indicator across all attach surfaces)

This rider holds the prescriptive constraints for the goal at `/Users/gdc/deadreckon/docs/goals/2026-06-05-0010-deadreckon-attach-tui-uniformity-goal.md`. It supersedes nothing in prior riders (Navigable `2026-05-29-1354`, Narrative Attach `2026-05-26-1546`, Release Trust `2026-06-01-1523`); their invariants hold. This rider unifies the chrome of the attach TUI across every surface you can attach to. The non-TUI rendering and one-shot prompts are handled by the sibling Uniform Surface rider (`2026-06-05-0009`); this rider CONSUMES its `Tone`/`display_width`/`kv_block` primitives and does not redefine them.

**All paths absolute.** Source root `/Users/gdc/deadreckon`. Runtime state under `/Users/gdc/.deadreckon` is out of scope. Tests use tempdirs and fixture projections.

## Posture (decided — do not redesign)

- **TUI presentation refactor on the production-release track.** No new product capability; no new attach target kinds.
- **Uniform chrome, mode-specific content.** Factor everything AROUND the panel into shared code; each mode keeps rendering its own body. Do NOT merge or change the content panels' data (run turn stream, plan child-ref tree, campaign feed, chain step graph, narrative projection).
- **No durable runtime schema changes.** `PipelineState`, plans, campaigns, chains, projections stay byte-identical on disk.
- **Byte-exact TUI render tests are the contract.** ratatui frames are asserted by characterization/golden tests; when a phase changes a frame, update the specific expected buffer in the same commit. Never delete a TUI test to pass.
- **Sequencing.** Prefer landing after Uniform Surface (`2026-06-05-0009`). If this lands first, reference `ui::Tone`/`display_width`/`kv_block` as they will exist; if a symbol is not yet present, add the minimal shared stub in `ui.rs` and note the dependency in the commit — do not fork a second copy inside `tui/`.
- **No `git push`.** Phased local commits only.
- **Edits stay inside `/Users/gdc/deadreckon`**, concentrated in `tui/`, `commands/attach.rs`, `commands/attach_runtime.rs`.

## Baseline and gaps (verified at HEAD)

Four attach surfaces, four key handlers:

- **Run** — `crates/deadreckon/src/tui/attach_state.rs` `handle_key` (attach_state.rs:82). The most complete: full paging + scroll indicator. THIS IS THE REFERENCE.
- **Plan** — inline loop in `crates/deadreckon/src/commands/attach.rs` (~attach.rs:606); lacks PgUp/PgDn/Home/End/g/G and a scroll indicator.
- **Campaign** — `crates/deadreckon/src/commands/attach_runtime.rs` `handle_campaign_key` (attach_runtime.rs:155); lacks paging keys; different footer.
- **Chain** — `crates/deadreckon/src/tui/render.rs` `handle_key` (render.rs:37); own glyph/footer; Unicode step glyphs without ASCII fallback.
- **Completion overlay** — `crates/deadreckon/src/commands/attach.rs` `handle_tui_completion_key` (attach.rs:925, dispatched at attach.rs:887): `[a]Apply`/`[b]Abandon` fire with NO confirm; `b` ambiguous (back-to-plan vs Abandon) by match order; `d` overloaded (docs-toggle vs Ctrl-D detach).
- **Narrative view** (`--view narrative`, supported for runs/plans/child refs per attach.rs:283) shares the loops; has its own split-width breakpoints (110 vs 100).

Concrete divergence sites: selection glyph `*` at render.rs:1011 and render.rs:2560 vs `>` elsewhere; four footer styles at render.rs:262/564/1156/1934; the brittle `parent_plan_footer.replace(...)` hack at render.rs:1963; scroll-position indicator only on run panels (render.rs:2564); `press Enter to return` prompts at attach.rs:396/651/981 (Enter only — no q/Esc); empty states leak filenames (campaign_feed at render.rs:526 prints `campaign-events.jsonl`/`plan-events.jsonl`); `chain_step_dot` glyphs (callers commands/chain/mod.rs:1746,2126) `○●◐✗↷◉↶` with no ASCII fallback.

## Unify rules (the spec)

- **One dispatcher.** Define a shared navigation core (a function or trait) handling the common keys — `↑↓`/`j k`, `Tab`/`BackTab`, `PgUp`/`PgDn`, `Home`/`End`, `g`/`G`, `Enter`, `q`/`Esc` — and call it from every surface. Each surface supplies only a `mode_key(key) -> handled: bool` hook for its mode-specific keys (chain step nav, campaign feed actions, plan child drill-in, completion actions). The run-panel handler's semantics are the reference; do not regress run behavior.
- **One glyph.** A single `selection_glyph()` (one highlight) replaces the `*`/`>` split at all call sites.
- **One footer.** A `footer(items)` builder produces the same layout and affordance strings for every surface; delete the `parent_plan_footer.replace()` hack. Footers list the active keys including `Esc`/`q`.
- **One scroll indicator.** A `scroll_indicator(first, last, total)` (`first-last/total`) rendered on EVERY list panel.
- **Confirm-before-destructive.** Apply/Abandon and any squash/apply require a two-step in-TUI confirm (key then explicit y/confirm). Resolve `b` (back vs Abandon) and `d` (docs vs detach) by distinct keys or an explicit modal, never by match order.
- **Uniform exit/return.** `Esc` = graceful leave/back, `Ctrl-D` = detach (run continues); the `press Enter to return` prompts also accept `q`/`Esc`; `Enter` on an unloadable child shows an "unavailable" notice instead of a silent no-op.
- **Friendly empty states.** Every empty panel carries a one-line next step and NEVER prints an internal filename. Narrative split breakpoints reconciled to one constant.
- **Windows.** `chain_step_dot` and any Unicode status marker gain an ASCII fallback set + legend under `--plain`/non-VT terminals.

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them fail; make the narrow change; `cargo fmt --check`, `cargo clippy --workspace --all-targets`, focused `cargo test` green; conventional-commit; one-line CHANGELOG naming the SHA. Use ratatui's `TestBackend`/buffer assertions for frame goldens; assert key handling by feeding `KeyEvent`s to the dispatcher.

### P1 — Extract the shared key dispatcher (run unchanged)

- Lift the common navigation handling out of `attach_state.rs handle_key` into a shared dispatcher with a per-mode hook; wire run-attach onto it. Run behavior must not change (existing run goldens stay green).
- Depth tests (tui): `shared_dispatcher_routes_common_navigation_keys`; `run_panel_key_handling_unchanged_after_extraction`.

### P2 — Plan surface onto the dispatcher

- Route the plan loop (attach.rs) through the dispatcher; add PgUp/PgDn/Home/End/g/G/BackTab parity.
- Depth tests: `plan_attach_supports_paging_keys`; `plan_attach_navigation_matches_run_reference`.

### P3 — Campaign surface onto the dispatcher

- Route `handle_campaign_key` through the dispatcher; add the missing paging keys.
- Depth tests: `campaign_attach_supports_paging_keys`; `campaign_attach_navigation_matches_run_reference`.

### P4 — Chain surface onto the dispatcher

- Route chain `render.rs handle_key` through the dispatcher; keep chain-specific step nav in the mode hook.
- Depth tests: `chain_attach_supports_paging_keys`; `chain_step_nav_still_works_via_mode_hook`.

### P5 — One selection glyph

- Replace `*` (render.rs:1011,2560) and the `>` variants with a single `selection_glyph()`.
- Depth tests: `selection_glyph_identical_across_surfaces`.

### P6 — One footer builder

- Replace the four footer styles (render.rs:262/564/1156/1934) with `footer(items)`; delete the `parent_plan_footer.replace()` hack (render.rs:1963); footers show the active keys including Esc/q.
- Depth tests: `footer_shape_identical_across_surfaces`; `parent_plan_footer_replace_hack_removed`.

### P7 — One scroll indicator on every list panel

- Render `scroll_indicator(first,last,total)` on plan/campaign/chain list panels (already on run, render.rs:2564).
- Depth tests: `scroll_indicator_present_on_all_list_panels`.

### P8 — Confirm-before-destructive

- Add a two-step in-TUI confirm before Apply and Abandon in `handle_tui_completion_key` (attach.rs:925); split the `b` back/Abandon and `d` docs/detach overloads into distinct keys or a modal.
- Depth tests: `apply_requires_confirmation_keystroke`; `abandon_requires_confirmation_keystroke`; `back_and_abandon_are_distinct_keys`.

### P9 — Uniform exit/return semantics

- Make the `press Enter to return` prompts (attach.rs:396/651/981) accept `q`/`Esc`; standardize `Esc` graceful vs `Ctrl-D` detach; show a notice when `Enter` hits an unloadable child.
- Depth tests: `return_prompt_accepts_q_and_esc`; `enter_on_unloadable_child_shows_notice`.

### P10 — Empty states, narrative consistency, Windows glyphs

- Give every empty panel a next step and stop leaking filenames (campaign_feed render.rs:526). Reconcile the narrative split breakpoints (110 vs 100) to one constant. Add ASCII fallbacks + legend for `chain_step_dot` glyphs in `--plain`/non-VT.
- Depth tests: `campaign_empty_state_has_hint_and_no_filename`; `narrative_split_breakpoint_is_single_constant`; `chain_step_glyphs_have_ascii_fallback_in_tui_plain`.

### P11 — Architecture doc + CHANGELOG (doc only; no depth test)

- Insert into `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:
  ```
  ## NN. Attach TUI Uniformity

  NN.1 The shared dispatcher + per-mode hook (run is the reference)
  NN.2 One glyph, one footer, one scroll indicator
  NN.3 Confirm-before-destructive and exit/return semantics
  NN.4 Empty-state and Windows-glyph rules
  ```
- If AS-BUILT has a "shipped vs thin" section, move "uniform attach controls across run/plan/campaign/chain" to shipped.
- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:
  ```
  ## Attach TUI Uniformity (production-release track) — 2026-06-05

  - One key dispatcher, glyph, footer, and scroll indicator across all attach surfaces.
  - Confirm before Apply/Abandon; uniform exit/return; ASCII glyph fallbacks.
  ```
- Log deferred items (live in-frame prompts, a TUI input widget) in `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

## Error-footer canonical pairs

| Surface signal | affordance shown |
|---|---|
| any list panel | `arrows/jk move, PgUp/PgDn page, g/G ends, Enter open, q/Esc back` |
| completion overlay | `a Apply, b Back, x Abandon — confirm with y` |
| unloadable child | `unavailable: <reason>` |

## Out of scope (explicitly not in this milestone)

- Non-TUI rendering and one-shot prompts — sibling slice `2026-06-05-0009`.
- New attach target kinds or content-panel data changes.
- Live prompts embedded inside an active ratatui frame, or an editable text widget (V1 candidates).
- Any `PipelineState`/plan/campaign/chain/projection schema change.

## Dependencies (Tier 1 / 2 / 3 policy)

- Tier 1: reuse `ui::Tone`/`display_width`/`kv_block` from the Uniform Surface slice; `unicode-width` already present via ratatui. No new crate.
- Tier 2 (log to `DEPENDENCIES.md`): none expected.
- Tier 3 (blocked): tui-input/tui-textarea/throbber-widgets-tui — no call site today; re-evaluate only when a real in-frame input/spinner is designed.

## Engineering invariants (do not violate)

- **No durable runtime schema changes.**
- **One depth test before each phase implementation;** a phase whose tests were never red is suspect.
- **TUI render tests are spec.** Frame changes are deliberate golden updates committed with the change.
- **Run behavior is the reference and must not regress** (P1 keeps run goldens green).
- **No content-panel data changes** — chrome only.
- **No silent scope expansion;** anything beyond P1-P11 goes to `V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth tests passing and a CHANGELOG entry naming the SHA.
- If a phase reveals a V1-architecture decision, log it in `V1-CANDIDATES.md` and continue.
