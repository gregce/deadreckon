# DeadReckon — Uniform Surface Rider (one styling layer, correct width, shared kv/table, hardened prompts)

This rider holds the prescriptive constraints for the goal at `/Users/gdc/deadreckon/docs/goals/2026-06-05-0009-deadreckon-uniform-surface-goal.md`. It supersedes nothing in prior riders (Release Trust `2026-06-01-1523`, Verdict Surface `2026-06-01-1417`, Navigable `2026-05-29-1354`); their invariants hold. This rider unifies how non-TUI commands render text and how one-shot prompts behave. The attach TUI is handled separately by `2026-06-05-0010-deadreckon-attach-tui-uniformity-rider.md`; do not touch `tui/`, `commands/attach*.rs`, or `attach_runtime.rs` rendering here except to consume the shared primitives this rider introduces.

**All paths absolute.** Source root `/Users/gdc/deadreckon`. Runtime state under `/Users/gdc/.deadreckon` is out of scope. Tests use tempdirs.

## Posture (decided — do not redesign)

- **Presentation refactor on the production-release track.** No new product capability beyond an interactive goal prompt for `deadreckon start`.
- **No durable runtime schema changes.** `PipelineState`, plans, campaigns, chains, receipts, markers stay byte-identical on disk.
- **Byte-exact render tests are the contract.** When a phase changes output, update the specific golden/characterization assertion in the same commit with the new expected bytes; never delete a render test to make output pass, never loosen `--plain`/`NO_COLOR` behavior.
- **No second terminal stack.** Only `unicode-width` and (optionally) `textwrap` may be added. No console/dialoguer/inquire/comfy-table/tabled/indicatif.
- **No `git push`.** Phased local commits only.
- **Edits stay inside `/Users/gdc/deadreckon`.** No `tui/` or attach rendering changes (separate goal).

## Baseline and gaps (verified at HEAD)

- `crates/deadreckon/src/ui.rs`: `enum Tone` (ui.rs:19), `TUI_PALETTE` (ui.rs:53), `enabled()` gate (ui.rs:74) honoring NO_COLOR/TERM=dumb/is_terminal/plain, `status_tone()` (ui.rs:143) with a silent `_ => Note` dim fallback; width via `.chars().count()`/`strip_ansi(...).chars().count()`.
- `crates/deadreckon/src/ui_card.rs`: a SEPARATE `enum Tone` (ui_card.rs:56, Neutral/Good/Warn/Bad/Dim), correct `pad_visible()` (ui_card.rs:120) and `visible_length()` (ui_card.rs:112) — but `visible_length` still counts chars, not display columns.
- Alignment BUG: hand-rolled tables apply Rust `{:<N}` to strings ALREADY wrapped in ANSI (e.g. `ui_id(...)`) in `commands/providers.rs`, `commands/inspection.rs`, `commands/plan.rs` preflight, the chain table, and lifecycle apply — `{:<N}` counts ESC bytes, so columns are short by the escape length whenever color is on. Providers also swaps id/symbol column order vs other tables.
- Hint BUG: `commands/campaign.rs` passes inverted hint polarity on its completion surface (~`no_hints` at campaign.rs:879-887) so `--no-hints`/`DEADRECKON_HINTS=0` is ignored; `commands/inspection.rs`, `commands/doc.rs` success surfaces, and five chain surfaces call `render_plain(false)` (always-on hints), ignoring the toggle.
- Casing/markers: UPPERCASE column headers in list/library vs lowercase keys elsewhere; `status` (main.rs `print_status_report`) mixes an auto-sized kv-block with hand-aligned colons (the `gate:` line ~main.rs:9933 and `scope artifacts:` ~:9954 break their sections). Five next-step markers coexist: `hint:`, `try:`, `Recommended`, proof_block `→`, campaign `Next`.
- Prompts `crates/deadreckon/src/prompt.rs`: `select_one_menu` (prompt.rs:105) raw-mode menu vs `select_one_line` plain path diverge; `select_index_from_digit` (prompt.rs:264) parses a SINGLE char 1-9 (no multi-digit, >9 unreachable) while line-mode `parse_select_answer` (prompt.rs:299) accepts multi-digit; Esc only cancels if a choice id=="cancel" (~prompt.rs:163); out-of-range digit silently ignored in menu mode; `render_select_menu` (prompt.rs:178) does `MoveUp(choices.len())` with no pagination (tall lists corrupt the screen). Count prompts in `commands/campaign.rs` and `commands/orchestrate.rs` do `prompt::open(...).parse::<u8>()` and hard-error the command on bad input.
- Wrapping: three divergent engines — `wrap_kv_value` (main.rs), `wrap_list_goal`/`wrap_words` (commands/inspection.rs), `wrap_campaign_words` (commands/campaign.rs at width ~88) — all char-count based.
- Windows: `chain_step_dot` (commands/chain/mod.rs:1746,2126 callers) emits `○●◐✗↷◉↶` with no ASCII fallback; duplicated terminal-width helpers (ui.rs fallback 120 vs prompt.rs fallback 119).

## Unify rules (the spec)

- **One Tone.** Define a single `Tone` in ui.rs with `to_ansi()` and `to_tui_color()`; derive `TUI_PALETTE` from it; `ui_card.rs` imports it (delete its enum). Replace `status_tone`'s open `_ => dim` with an explicit `Status` enum mapped exhaustively, so a new status is a compile error, not an invisible dim line.
- **One width.** `display_width(&str)` = `UnicodeWidthStr::width(&strip_ansi(s))`. Route every visible-width site through it: ui.rs truncate/replace, ui_card `visible_length`/`pad_visible`/`truncate_visible`, verdict_surface, proof_block, and all hand-rolled tables. FORBID `{:<N}`/`{:>N}` on styled strings — pad with `pad_visible(display_width)`.
- **Two columnar primitives.** `kv_block(rows)` auto-sizes the key column via `display_width`, emits lowercase `key: value`. `columns(headers, rows)` lowercase headers, display-width padding, terminal-width fit, ellipsis. Both honor `enabled()`/`--plain`.
- **One next-step marker.** Pick one (recommend the VerdictSurface `Recommended`/`try:` pair already most common); route the others through it. Bare `println!("cancelled")` and raw failure dumps become a cancel/error VerdictSurface with a Recommended step.
- **Prompt parity.** Menu mode and line mode share the `?` Tone marker, row layout, affordance text, multi-digit number accumulation, the same validation messages, always-available Esc cancel (auto-inject an implicit cancel sentinel if the caller supplied none), and pagination or line-mode fallback when choices exceed terminal height.

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them fail; make the narrow change; `cargo fmt --check`, `cargo clippy --workspace --all-targets`, focused `cargo test` green; conventional-commit; one-line CHANGELOG naming the SHA.

### P1 — display_width on unicode-width

- Add `unicode-width = "0.2"` (aligns with ratatui's resolved 0.2). Introduce `display_width()`; migrate EVERY `.chars().count()`/`strip_ansi(...).chars().count()` width site (ui.rs, ui_card.rs, prompt.rs, commands/inspection.rs, main.rs, commands/campaign.rs, verdict_surface, proof_block) including the per-char truncators.
- Depth tests (ui.rs / ui_card.rs): `display_width_strips_ansi_then_measures_columns`; `display_width_counts_wide_cjk_as_two`; `truncate_visible_does_not_overflow_on_wide_chars`.

### P2 — One Tone + explicit Status

- Collapse the two `Tone` enums into one in ui.rs (`to_ansi` + `to_tui_color`); derive `TUI_PALETTE`; `ui_card.rs` imports it. Replace `status_tone`'s silent fallback with an exhaustive `Status` enum.
- Depth tests: `same_status_maps_to_same_tone_in_line_and_tui`; `unknown_status_is_explicit_not_silently_dimmed`.

### P3 — Fix the ANSI-padding alignment bug

- Promote `pad_visible()` to a shared util on `display_width`. Replace every `{:<N}`/`{:>N}` applied to `ui_id()`/`ui_status()`-styled strings in providers/inspection/plan/chain/lifecycle; fix the providers id/symbol column-order swap. Add a guard test/clippy-style check that fails if a format-pad is applied to a string containing an ESC byte.
- Depth tests: `colored_column_aligns_same_with_color_on_and_off`; `format_pad_over_ansi_is_rejected`.

### P4 — Hint polarity + discipline

- Fix `commands/campaign.rs` inverted polarity to `!completion_hints_enabled(no_hints)`; change every `render_plain(false)` in inspection/doc/chain to the toggle-respecting form.
- Depth tests: `no_hints_suppresses_campaign_completion_hints`; `no_hints_suppresses_inspection_doc_chain_hints`.

### P5 — Shared kv_block

- Build `kv_block`; migrate `print_status_report`, run-started, guided-start, library show, doctor evidence, campaign facts, chain header onto it; delete hand-aligned colon blocks and magic widths; lowercase keys.
- Depth tests: `kv_block_aligns_dynamic_key_column`; `status_report_uses_kv_block_no_manual_colons`.

### P6 — Shared columns table

- Build `columns`; migrate list, library list, providers, plan preflight, chain table; lowercase headers; terminal fit + ellipsis.
- Depth tests: `columns_pads_by_display_width_with_color`; `list_and_library_use_shared_columns`.

### P7 — Prompt menu hardening

- Multi-digit accumulation in menu mode (short digit buffer, Enter/timeout commit) matching `parse_select_answer`; identical menu/line rendering + affordance; Esc always cancels (auto-inject implicit cancel) advertised in the footer; out-of-range/non-numeric feedback in menu mode; pagination or line-mode fallback when `choices.len()` exceeds terminal height; reconcile the duplicated width helpers (ui.rs 120 vs prompt.rs 119) into one.
- Depth tests: `menu_mode_selects_choice_above_nine_by_number`; `esc_cancels_without_explicit_cancel_choice`; `tall_menu_falls_back_or_paginates`; `menu_mode_reports_out_of_range`.

### P8 — Validated ask_number + count prompts

- Add `prompt::ask_number(range)` (empty->default, non-numeric->reprompt, range-check before accept). Route campaign child/regenerate count and orchestrate `prompt_child_count` through it; they must LOOP, not hard-error the command.
- Depth tests: `ask_number_reprompts_on_non_numeric_and_out_of_range`; `campaign_count_prompt_loops_instead_of_exiting`.

### P9 — Confirm modality + interactive goal entry

- Standardize: binary decision = `prompt::confirm` (y/n); >2-way choice = `prompt::select_one`. Align run/finish/campaign so a user does not hit an arrow menu in one step and y/n in the next. Allow `deadreckon start` with no goal to prompt via `prompt::open` when tty and not `--yes/--json/--plain/--quiet`; when prompts are suppressed print a one-line notice.
- Depth tests: `start_without_goal_prompts_when_tty`; `start_without_goal_prints_notice_when_prompts_suppressed`.

### P10 — Next-step vocabulary, wrapping, Windows fallbacks (cross-cutting friendliness)

- Replace bare `println!("cancelled")`/raw failure dumps (run.rs, plan.rs, doc.rs, chain/mod.rs, acceptance.rs) with a shared cancel/error VerdictSurface carrying one Recommended step; pick ONE next-step marker and route the five into it. Adopt `textwrap` (or one hand-rolled wrapper on `display_width`) and collapse `wrap_kv_value`/`wrap_list_goal`/`wrap_campaign_words` into one engine, wrapping RAW text before `ansi_wrap`. Add ASCII fallbacks + a legend for `chain_step_dot` glyphs under `--plain`/non-TTY (these render in non-TUI chain output).
- Depth tests: `cancelled_run_renders_surface_with_next_step`; `one_wrap_engine_used_for_kv_list_campaign`; `chain_step_glyphs_have_ascii_fallback_in_plain`.

### P11 — Architecture doc + CHANGELOG (doc only; no depth test)

- Insert into `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:
  ```
  ## NN. Uniform Surface

  NN.1 One Tone, one display_width, the enabled() color policy
  NN.2 Shared kv_block / columns primitives and the no-format-pad-over-ANSI rule
  NN.3 Prompt engine contract (menu/line parity, multi-digit, Esc, ask_number)
  NN.4 Next-step marker and wrapping
  ```
- If AS-BUILT has a "shipped vs thin" section, move "uniform CLI rendering" and "hardened one-shot prompts" to shipped; note the attach TUI is handled by the attach-tui-uniformity slice.
- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:
  ```
  ## Uniform Surface (production-release track) — 2026-06-05

  - One Tone + display_width; shared kv_block/columns; fixed ANSI-padding alignment bug.
  - Honored --no-hints everywhere; hardened prompt menus (multi-digit, Esc, ask_number).
  ```
- Log deferred items (comfy-table adoption, anstyle-wincon Windows path) in `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| `format-pad applied to a styled string` | `pad with pad_visible(display_width(s), n)` |
| `unknown status word` | `add the variant to the Status enum` |
| `count out of range` | `enter a number in 2..=6` |
| `menu choice out of range` | `choose 1-N or press Esc to cancel` |

## Out of scope (explicitly not in this milestone)

- The attach TUI (`tui/`, `commands/attach*.rs`, `attach_runtime.rs`) chrome — separate goal `2026-06-05-0010`.
- Adopting comfy-table/tabled or any external prompt/styling crate.
- anstyle-wincon / legacy-Windows console-API color translation (V1 candidate).
- An editable multi-char input field (no call site today).
- Any `PipelineState`/plan/campaign/chain/receipt schema change.

## Dependencies (Tier 1 / 2 / 3 policy)

- Tier 1: `unicode-width` (already transitive via ratatui; direct dep is zero added weight); `textwrap` (MIT; optional — adds unicode-linebreak + smawk; or hand-roll one wrapper on `display_width`).
- Tier 2 (log to `DEPENDENCIES.md`): none expected.
- Tier 3 (blocked): console/dialoguer/inquire/comfy-table/indicatif — they introduce a second terminal stack or theme and break byte-exact tests.

## Engineering invariants (do not violate)

- **No durable runtime schema changes.**
- **One depth test before each phase implementation;** a phase whose tests were never red is suspect.
- **Render tests are spec.** Whitespace and marker changes are deliberate golden updates, committed with the change.
- **No `{:<N}` over ANSI** (the guard test enforces it).
- **No silent scope expansion;** anything beyond P1-P11 goes to `V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth tests passing and a CHANGELOG entry naming the SHA.
- If a phase reveals a V1-architecture decision, log it in `V1-CANDIDATES.md` and continue.
