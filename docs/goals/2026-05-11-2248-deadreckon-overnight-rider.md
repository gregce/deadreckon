# deadreckon — Overnight Rider (caffeinate + cards)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-11-2248-deadreckon-overnight-goal.md`.
It supersedes nothing in prior riders (2026-05-10-build,
2026-05-11-{audit-harden, autonomous-chain, codebase, doc-depth,
orchestrate, primary-flow, provider-registry, robust, self-documenting,
usability}) — their invariants still apply. This rider adds the card
vocabulary, caffeinate / systemd-inhibit sleep prevention, and the
unattended-git hardening pair that makes a pre-bedtime `run` reliably
outlive a closed laptop.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`~/.deadreckon/` (smoke runtime under
`/Users/gdc/deadreckon/.deadreckon-smoke`).

## Posture (decided — do not redesign)

- **Maturity stays `alpha`.**
- **No `PipelineState` schema changes.** Sleep-prevention state lives
  in `working/.deadreckon/sleep-prevention.json`. The `~`-prefix
  spend flags read directly from `spend.jsonl`.
- **Sleep prevention is opt-out, not opt-in, when the run is
  interactive.** TTY-attached `run` defaults to `--prevent-sleep on`.
  Non-TTY (CI, pipes) defaults to `off`. Config can pin either side.
- **Card primitives live in one place** —
  `crates/deadreckon/src/ui_card.rs`. All user-facing output
  (preview, exit, status, show, list, attach completion footer) flows
  through that module. No phase may bypass the renderer with a
  freestanding `println!` for surface content.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Windows native sleep prevention, tiered failure
  handling, `--stop-when`, the notes-buffer-between-turns idea,
  conventional-commit per-turn preset, permanent-error stderr
  classification, and host SKILL.md all stay out of scope; if a phase
  surfaces one, log to `docs/V1-CANDIDATES.md` and continue.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

### Overlap with peer riders — land non-conflicting

- **Audit-harden P7 (`doctor` exhaustive)** adds opt-in provider-ping
  rows. This rider adds two sleep-prevention rows to `doctor`. Both
  appear under the existing "Runtime checks" section; do not rename
  the section; append rows.
- **Audit-harden P10 (`help` / `status` polish)** retargets help-text
  grouping and `status` summary content. This rider replaces the
  *rendering* of `status`/`show`/`list` onto the card primitives but
  keeps the content surfaces audit-harden specifies. If audit-harden
  lands first, P4 here is a render swap on top of its data shape. If
  this rider lands first, audit-harden P10 must call into `ui_card`
  rather than re-introduce ad-hoc formatting.
- **Autonomous-chain** introduces `chain attach` and a step-timeline
  TUI. The chain TUI is out of scope here; its eventual exit summary
  should reuse this rider's card primitives but that work is logged
  in the chain rider, not this one.

## Data model (files, not fields)

### `working/.deadreckon/sleep-prevention.json`

Written when sleep prevention arms; deleted when it disarms. On crash
recovery, deadreckon reaps the recorded pid if alive and removes the
file regardless.

```json
{
  "mode": "caffeinate",
  "pid": 51234,
  "armed_at": "2026-05-11T22:15:32Z",
  "inhibitor_binary": "/usr/bin/caffeinate",
  "reason": "deadreckon overnight run",
  "skip_reason": null
}
```

Field reference:

- `mode` — one of `"caffeinate"`, `"systemd-inhibit"`, `"none"`,
  `"unsupported"`.
- `pid` — pid of the inhibitor child (`caffeinate`) or the re-exec'd
  self under `systemd-inhibit`; `null` when `mode` is `"none"` or
  `"unsupported"`.
- `inhibitor_binary` — resolved path to the binary; `null` when no
  inhibitor was spawned.
- `skip_reason` — one of `"non-tty"`, `"user-disabled"`,
  `"unavailable"`, `"already-inhibited"`, `"unsupported"`, or `null`
  when armed.

### `~/.deadreckon/config.toml` additions

```toml
[defaults]
prevent_sleep = "auto"  # "auto" | "on" | "off"
plain = false           # global default for the --plain flag
```

## Card primitives (the spec)

```rust
// crates/deadreckon/src/ui_card.rs

pub struct Card {
    pub title: TitleLine,
    pub subtitle: Option<String>,
    pub sections: Vec<Section>,
    pub hints: Vec<HintLine>,
}

pub struct TitleLine {
    pub glyph: TitleGlyph,
    pub label: String,
}

pub enum TitleGlyph {
    Success,    // ✦ (or "*" under --plain)
    Stopped,    // × (or "x")
    Paused,     // ⧖ (or "~")
    Failed,     // ⊘ (or "!")
    Preview,    // ▸ (or ">")
}

pub enum Section {
    Metric { label: String, columns: Vec<MetricColumn> },
    KeyValue { rows: Vec<(String, String)> },
    Command { label: String, command: String },
    Blank,
}

pub struct MetricColumn {
    pub value: String,
    pub tone: Tone,
}

pub enum Tone { Neutral, Good, Warn, Bad, Dim }

pub struct HintLine {
    pub label: String,
    pub command: String,
}

pub struct CardOptions {
    pub color: bool,
    pub plain: bool,
    pub terminal_columns: Option<usize>,
    pub no_color_env: bool,
}

pub fn render_card(card: &Card, opts: &CardOptions) -> String;
pub fn visible_length(text: &str) -> usize;
pub fn truncate_visible(text: &str, width: usize) -> String;
pub fn pad_visible(text: &str, width: usize) -> String;
```

### Render contract (depth-tested; do not drift)

- `render_card` is deterministic for fixed inputs across platforms.
  No ANSI shell autodetection inside the function — all detection
  resolves in the caller and is passed via `CardOptions`.
- `visible_length(text)` equals `text.chars().filter(non_ansi).count()`
  for every escape-sequence shape used (`\x1b[...m`).
- `truncate_visible` ellipsis is `…`; the truncated string always
  ends with `\x1b[0m` if any style was active. Under `plain`, the
  ellipsis is `...` and no reset is appended.
- `plain = true` strips both color and box-drawing; borders become
  `+-+|` ASCII art; bullets become `-`.
- `terminal_columns = None` falls back to 80. The resolved width is
  always `max(MIN_CARD_WIDTH, content_width + 4)` capped at the
  terminal width. Below 40 columns, the renderer switches to a
  single-column ASCII fallback with no borders.
- Golden fixtures under
  `/Users/gdc/deadreckon/crates/deadreckon/tests/fixtures/cards/` are
  byte-compared (ANSI-stripped). Whitespace changes are spec
  changes.

## Sleep prevention (the spec)

```rust
// crates/deadreckon/src/sleep.rs

pub enum SleepPrefs { Auto, On, Off }

pub enum SleepPrevention {
    Active { handle: SleepHandle },
    Skipped { reason: SkipReason },
    Reexeced { exit_code: i32 },
}

pub enum SkipReason {
    NonTty,
    UserDisabled,
    UnavailableBinary,
    AlreadyInhibited,
    Unsupported,
}

pub fn arm(prefs: SleepPrefs, run_root: &Path) -> Result<SleepPrevention>;

pub struct SleepHandle {
    // Drop reaps the inhibitor child (if any) and removes
    // working/.deadreckon/sleep-prevention.json.
}
```

### macOS spec

- Spawn `caffeinate -di` as a child process. Detached process group so
  Ctrl-C in the parent doesn't terminate the inhibitor before deadreckon
  cleans up.
- Write `sleep-prevention.json` with the child pid + binary path.
- On `Drop`, send `SIGTERM`, wait up to 500 ms, then `SIGKILL` if still
  alive. Remove the JSON file.
- If `caffeinate` is missing from `$PATH` and `/usr/bin/caffeinate`:
  record `mode = "unsupported"`, `skip_reason = "unavailable"`, log a
  warning, continue without prevention.

### Linux spec

- If `$DEADRECKON_SLEEP_REEXEC_READY_PATH` is set, we are already the
  re-exec'd child: validate the path against the trusted-path
  invariants below, then write `"ready\n"` with `O_WRONLY | O_CREAT |
  O_EXCL` flags (`wx` semantics). If validation fails or the write
  fails, log and continue the run; do not abort.
- Otherwise, `mkdtemp` under `$TMPDIR` (or `/tmp` if unset) with prefix
  `deadreckon-sleep-`, set
  `DEADRECKON_SLEEP_REEXEC_READY_PATH=<dir>/reexec-ready` in the child
  env, and re-exec self under `systemd-inhibit --what=sleep
  --who=deadreckon --why="<reason>" --mode=block <self> <argv>`. Wait
  up to 5 s for the ready file or child exit. If timeout, kill the
  child and continue without prevention (skip reason `unavailable`).
- Trusted-path invariants (both sides):
  - `basename(path) == "reexec-ready"`
  - `basename(dirname(path)).starts_with("deadreckon-sleep-")`
  - `dirname(dirname(path)) == realpath(tmpdir())`
- If `systemd-inhibit` is not on `$PATH`: `arm` returns `Skipped {
  reason: UnavailableBinary }`. `doctor` includes
  `try: sudo apt install systemd`.

### Windows spec

- Always `Skipped { reason: Unsupported }`. `doctor` adds a row noting
  that native Windows prevention is logged in
  `docs/V1-CANDIDATES.md`.

## Verb signatures (touched)

```
deadreckon run <goal>
    [--prevent-sleep <auto|on|off>]   # default = config (auto)
    [--plain]                          # ASCII + no color globally for this invocation

deadreckon resume <id>    [--plain]
deadreckon kill    <id>   [--plain]
deadreckon apply   <id>   [--plain]
deadreckon attach  <id>   [--plain]
deadreckon status         [--plain]
deadreckon show    <id>   [--plain]
deadreckon list           [--plain]
deadreckon doctor                       # adds the sleep-prevention check rows
```

`--plain` propagates to every card the verb renders.

### Refusal cases

| Refusal | `try:` |
|---|---|
| `--prevent-sleep on` on Windows | `try: --prevent-sleep off (Windows native prevention is a V1 candidate)` |
| `--prevent-sleep on` without inhibitor binary | `try: brew install caffeinate` (macOS) or `try: sudo apt install systemd` (Linux) |
| `--plain` with invalid color override | (silently strips; never refuses) |
| Width < 40 and `--plain` not set | (auto-falls back; never refuses) |
| `sleep-prevention.json` corrupt | `try: rm working/.deadreckon/sleep-prevention.json` |

## Detection rules

| Input | Resolved sleep mode |
|---|---|
| `--prevent-sleep off` | `none` |
| `--prevent-sleep on` (TTY+macOS+caffeinate) | `caffeinate` |
| `--prevent-sleep on` (TTY+Linux+systemd-inhibit) | `systemd-inhibit` |
| `--prevent-sleep on` (TTY+Linux, no systemd-inhibit) | `unsupported` (warn, continue) |
| `--prevent-sleep on` (Windows) | `unsupported` (warn, continue) |
| `--prevent-sleep on` (non-TTY) | platform-default (overrides auto-off) |
| `--prevent-sleep auto` (TTY) | platform-default |
| `--prevent-sleep auto` (non-TTY) | `none` |
| `defaults.prevent_sleep = "on"` (no flag, non-TTY) | platform-default |

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them
fail; implement; green on
`cargo build --release && cargo test --workspace && cargo clippy
--workspace -- -D warnings && cargo fmt --check`; conventional-commit
local commit; one-line CHANGELOG entry.

### P1 — Card primitives module

- Add `crates/deadreckon/src/ui_card.rs` with the `Card` / `Section` /
  `CardOptions` types and the `render_card`, `visible_length`,
  `pad_visible`, `truncate_visible` functions.
- Re-export under `mod ui_card;` in `crates/deadreckon/src/main.rs`.
  No callers yet; pure module + tests.
- Commit golden fixtures used by P2/P3 fixture tests into
  `crates/deadreckon/tests/fixtures/cards/`.

Depth tests (in `crates/deadreckon/tests/ui_card.rs`):
- `card_renders_fixed_layout_for_known_input`
- `card_truncates_with_ellipsis_preserving_active_ansi`
- `card_plain_mode_strips_color_and_box_drawing`
- `card_visible_length_skips_ansi_escape_sequences`
- `card_resolves_width_with_terminal_fallback_to_eighty`
- `card_below_forty_cols_falls_back_to_single_column_ascii`

### P2 — Exit summary card

- Add `crates/deadreckon/src/cards/exit_summary.rs` that builds a
  `Card` from `(PipelineState, SpendSummary, BranchDiffSummary,
  OutcomeKind)` where `OutcomeKind` is `Completed | Paused | Killed |
  Failed`.
- Print at the tail of `run`, `resume`, `kill`, and `apply`.
- Section order: title glyph + outcome label; subtitle
  `<provider/agent> worked for <elapsed> on <branch>` or the stopped
  variant; `turns` (total / done / refused); `tokens` (in / out);
  `spend` (USD with optional `~`); `wall` (when any turn subscription);
  `branch diff` (+lines / −lines / N files); `files` (added /
  updated / deleted); `gate` (signed / acceptance check kind result);
  `notes` (working dir); `proof` (`proofs/turn-acceptance.json` path);
  `next steps` (`deadreckon attach <id>`, `deadreckon show <id>`,
  `deadreckon apply <id>` when applicable for the outcome).

Depth tests (in `crates/deadreckon/tests/cards_exit_summary.rs`):
- `exit_summary_completed_run_includes_attach_show_apply_hints`
- `exit_summary_paused_run_uses_paused_glyph_and_resume_hint`
- `exit_summary_killed_run_uses_stopped_glyph_and_reason`
- `exit_summary_failed_run_uses_failed_glyph_and_logs_hint`
- `exit_summary_subscription_turn_marks_spend_with_tilde`
- `exit_summary_no_branch_diff_when_codebase_is_fresh`
- `exit_summary_matches_golden_fixture_for_three_turn_smoke_run`

### P3 — Pre-run preview card

- Refactor the existing `--preview` text path in the `run` command
  onto `ui_card`. Same aesthetic as P2's exit card; `Preview` glyph.
- Section order: title (▸ "deadreckon run preview"); subtitle (the
  goal, truncated to width); `mode` (worktree/copy/in-place/fresh);
  `branch` / `base ref` / `worktree path`; `provider` / `model` /
  `skill`; `caps` (max-spend / max-wall-seconds / max-turns); `sleep`
  (mode + binary or skip reason); `confirmation` (only when
  `--max-spend > $50`); `next steps` (the verbatim `deadreckon run ...`
  command + `--yes` hint).
- `--preview` flag still exits 0 after rendering. Without it, preview
  renders, then the run continues.

Depth tests (in `crates/deadreckon/tests/cards_preview.rs`):
- `preview_card_shows_sleep_mode_row_for_caffeinate`
- `preview_card_shows_sleep_skip_reason_when_non_tty`
- `preview_card_exits_zero_when_preview_flag_set`
- `preview_card_shows_confirmation_row_when_max_spend_above_fifty`
- `preview_card_aesthetic_matches_exit_card_fixture`

### P4 — `status` / `show` / `list` harmonization

- `deadreckon` no-args (and `status` / `next`) → card with: project
  scope, latest run id + status + outcome glyph, sleep mode (if
  alive), next-action hints.
- `show <id>` → card with: run id, mode, branch, provider/model, caps,
  turns/spend/wall, gate, lineage (extended-from /
  materialized-to), doc paths.
- `list` (compact) → table-card with one row per run: short id, goal
  (truncated to width), status glyph, turns, spend, age.
- `list --full` keeps the existing rich (non-card) output for scripts.
  `--plain` forces ASCII even when interactive.
- Coordination with audit-harden P10: audit-harden defines the
  *content* of status/list polish; this rider replaces the *rendering*
  with `ui_card`. If audit-harden lands first, this phase swaps the
  renderer; if this rider lands first, audit-harden P10 must call
  into `ui_card`.

Depth tests (in `crates/deadreckon/tests/cards_status.rs`):
- `status_card_shows_sleep_mode_when_active`
- `show_card_includes_lineage_section_when_extend_parent`
- `list_compact_card_table_truncates_goal_with_ellipsis`
- `list_full_keeps_old_layout_for_scripts`

### P5 — `--prevent-sleep` on macOS (caffeinate)

- Add `crates/deadreckon/src/sleep.rs` with `arm` / `SleepPrevention`
  / `SleepHandle` / `SkipReason`. Internal modules `mac.rs`, `linux.rs`,
  `windows.rs`, `tty.rs` for platform splits.
- Wire into the `run` command immediately after preview confirmation
  and immediately before `run_turn_loop`. The `SleepHandle` is RAII —
  bound to the run loop's lifetime, dropped on every exit path
  (completion, pause-at-cap, killed, failed, panic).
- Write `working/.deadreckon/sleep-prevention.json` with the spawned
  caffeinate pid.
- On `Drop`, send `SIGTERM`, wait 500 ms, then `SIGKILL` if still
  alive. Remove the JSON file.
- `doctor` reports `caffeinate present at <path>` or a `try:` line if
  missing. Coordination with audit-harden P7: append rows under the
  existing "Runtime checks" section.

Depth tests (in `crates/deadreckon/tests/sleep_macos.rs`,
gated on `#[cfg(target_os = "macos")]`):
- `prevent_sleep_macos_spawns_caffeinate_child_when_tty`
- `prevent_sleep_macos_drop_reaps_caffeinate_within_500ms`
- `prevent_sleep_macos_writes_and_removes_metadata_file`
- `prevent_sleep_off_does_not_spawn_caffeinate`
- `prevent_sleep_auto_skips_when_non_tty`
- `prevent_sleep_macos_handles_missing_binary_with_unavailable_skip`

### P6 — `--prevent-sleep` on Linux (systemd-inhibit + handshake)

- Implement the re-exec branch in `linux.rs`: if
  `DEADRECKON_SLEEP_REEXEC_READY_PATH` is set, validate the path
  against the trusted-path invariants, then write `"ready\n"` with
  `O_WRONLY | O_CREAT | O_EXCL`. If not, mkdtemp under `$TMPDIR`,
  set the env, and re-exec self under `systemd-inhibit ...`. Wait up
  to 5 s for the ready file or child exit.
- The trusted-path invariants are enforced symmetrically.
- Windows: `arm` returns `Skipped { reason: Unsupported }`. `doctor`
  notes the V1 candidate.

Depth tests (in `crates/deadreckon/tests/sleep_linux.rs`,
gated on `#[cfg(target_os = "linux")]` plus a portable subset that
runs everywhere for path-validation logic):
- `prevent_sleep_linux_writes_ready_file_when_under_inhibitor`
- `prevent_sleep_linux_refuses_untrusted_ready_path`
- `prevent_sleep_linux_falls_back_when_systemd_inhibit_missing`
- `prevent_sleep_linux_timeout_after_five_seconds_does_not_hang_run`
- `prevent_sleep_windows_skipped_with_unsupported_reason` (portable)
- `prevent_sleep_trusted_path_validator_rejects_outside_tmp` (portable)

### P7 — Unattended-git hardening

- Audit every `Command::new("git")` in `crates/deadreckon-core/` and
  `crates/deadreckon/`. Route through a single helper
  `crates/deadreckon-core/src/git.rs::run_git(args, cwd) -> Result<...>`
  that:
  - sets `GIT_TERMINAL_PROMPT=0` in the child env;
  - inserts `-c commit.gpgsign=false -c tag.gpgsign=false
    -c gpg.format=` immediately after `git` for any argv whose first
    positional verb is one of `commit`, `merge`, `cherry-pick`,
    `rebase`, `tag`, `am`, `revert`;
  - leaves `git status` / `git rev-parse` / `git log` / `git diff` /
    `git symbolic-ref` / `git config` / `git worktree` / `git
    checkout` argv unchanged.
- Existing call sites to migrate (non-exhaustive):
  - `commit_worktree_turn` in
    `/Users/gdc/deadreckon/crates/deadreckon-core/src/turn_loop.rs:1035`
  - worktree creation/removal in `crates/deadreckon-core/src/codebase.rs`
  - `apply` / `abandon` / `cleanup` paths in
    `crates/deadreckon/src/main.rs`

Depth tests (in `crates/deadreckon-core/tests/git_hardening.rs`):
- `git_run_exports_git_terminal_prompt_zero_in_env`
- `git_commit_args_include_commit_gpgsign_false`
- `git_status_args_do_not_include_commit_gpgsign_false`
- `worktree_turn_commit_succeeds_under_fake_gpg_that_would_hang`
- `apply_commit_succeeds_under_global_signing_config`
- `git_invocation_grep_finds_no_raw_command_new_outside_helper` (a
  grep-style guard over `crates/**/*.rs`, excluding `git.rs` and
  vendored test fixtures)

### P8 — Honest spend display

- Add a `SpendSummary` aggregator in `crates/deadreckon-core/src/state.rs`
  (helper function, not a struct field) that scans `spend.jsonl` and
  reports `total_usd`, `any_subscription_turn`, `any_estimated_turn`.
- The exit / status / show cards render the spend column as
  `~$X.XX` whenever `any_subscription_turn || any_estimated_turn`.
  The number itself remains computed honestly.
- Subscription detection reads the existing `subscription: true` field
  in `SpendRecord`. Estimation flag is plumbed for future CLI
  subscription paths and defaults to `false`; no immediate user
  surface change beyond the `~` rendering.

Depth tests (in `crates/deadreckon-core/tests/spend_summary.rs`):
- `spend_summary_marks_tilde_when_any_turn_subscription`
- `spend_summary_no_tilde_when_all_http_priced`
- `spend_summary_tilde_persists_after_resume_via_jsonl_replay`
- `spend_summary_total_unchanged_by_tilde_flag`

### P9 — Friendliness pass: `--plain` / `NO_COLOR` / width fallback

- Wire `--plain` through every verb that emits cards. Resolution
  order: flag > `NO_COLOR` env > `defaults.plain` config > false.
- Width resolution: `--width <cols>` (test-only hidden flag) >
  `COLUMNS` env > `crossterm::terminal::size()` > 80. Below 40 cols
  the cards switch to single-column ASCII (no borders, dashes for
  separators).
- Every refusal footer in card-emitting verbs ends with a `try:` line.

Depth tests (in `crates/deadreckon/tests/cards_friendliness.rs`):
- `plain_flag_strips_color_and_box_drawing_globally`
- `no_color_env_implies_plain_for_cards`
- `width_below_forty_uses_single_column_ascii_fallback`
- `every_refusal_footer_ends_with_try_line` (parameterized over the
  refusal-footer table in this rider)

### P10 — `attach` exit inline card

- When the run completes while `attach` is still attached, replace
  the current "completion action footer" prompt with an inline render
  of the same exit card. Ctrl-D detach must still work mid-run.
- The TUI exits the ratatui alternate screen first, then prints the
  card to stdout (not inside the alternate screen).

Depth tests (in `crates/deadreckon/tests/attach_inline_card.rs`):
- `attach_exit_renders_completion_card_on_run_completed_event`
- `attach_exit_preserves_ctrl_d_detach_during_running_state`

### P11 — Architecture doc update + CHANGELOG (doc only; no depth test)

- Insert a new top-level section into
  `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:

  ```
  ## 26. Overnight UX

  26.1 Card vocabulary
  26.2 Sleep prevention (macOS / Linux / Windows-unsupported)
  26.3 Unattended-git hardening
  26.4 Honest spend display
  ```

- Update "## 22. What's Built vs Scaffolding-Thin": add to the
  "Built and reliable" column:
  - `--prevent-sleep <auto|on|off>` on macOS (caffeinate) and Linux
    (systemd-inhibit with tmpfs ready-path handshake), Windows skip.
  - Unified `ui_card` renderer for preview / exit / status / show /
    list / attach footer; `--plain`, `NO_COLOR`, terminal-width fallback.
  - Unattended-git hardening (`GIT_TERMINAL_PROMPT=0`, GPG signing
    disabled for commit/tag/merge/rebase/cherry-pick).
  - Honest spend with sticky `~` for subscription/estimated turns.
- Do not remove any thin items the audit-harden rider is closing in
  parallel.

- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:

  ```
  ## Overnight UX (alpha) — 2026-05-11

  - Added a unified `ui_card` renderer used by run preview, exit summary, status, show, list, and the attach completion footer; ANSI-safe, terminal-width capped, `--plain` and `NO_COLOR` aware.
  - Added `--prevent-sleep <auto|on|off>` with platform-native inhibitors: macOS `caffeinate -di`, Linux `systemd-inhibit` with a tmpfs ready-path handshake; Windows reports unsupported via `doctor`.
  - Hardened unattended git: every commit-family invocation in `deadreckon-core` and `deadreckon` exports `GIT_TERMINAL_PROMPT=0` and disables commit/tag GPG signing so global signing can no longer hang per-turn commits on pinentry.
  - Marked spend totals with `~` whenever any turn was subscription-priced or estimated, so the displayed dollar figure never overstates precision.
  - Wired `--plain`, `NO_COLOR`, and a width fallback through every card-emitting verb.
  ```

- Optional asciinema cast under
  `/Users/gdc/deadreckon/demo-overnight.cast` showing a smoke run with
  preview → caffeinate spawn → exit card.

## Integration matrix (cards × verbs)

| Verb | Preview card | Exit card | Status card | Inline footer | `--plain` | Sleep row |
|---|---|---|---|---|---|---|
| `run` | yes | yes (tail) | n/a | n/a | yes | yes (preview + exit) |
| `resume` | minimal | yes (tail) | n/a | n/a | yes | yes (exit) |
| `kill` | n/a | yes (tail) | n/a | n/a | yes | no |
| `apply` | yes (rebase preview) | yes (tail) | n/a | n/a | yes | no |
| `status` / `next` (no args) | n/a | n/a | yes | n/a | yes | yes when alive |
| `show <id>` | n/a | n/a | yes (rich) | n/a | yes | yes when alive |
| `list` (compact) | n/a | n/a | yes (table) | n/a | yes | no |
| `list --full` | n/a | n/a | (legacy text) | n/a | (no-op) | no |
| `attach` | n/a | yes (on RunCompleted) | n/a | yes (mid-run) | yes | yes |
| `doctor` | n/a | n/a | yes (sections) | n/a | yes | no (reports availability) |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| `caffeinate not found on macOS` | `try: macOS bundles caffeinate; check $PATH or run "/usr/bin/caffeinate -di"` |
| `systemd-inhibit not found on Linux` | `try: sudo apt install systemd` |
| `sleep-prevention.json points at dead pid <N>` | `try: rm working/.deadreckon/sleep-prevention.json` |
| `untrusted DEADRECKON_SLEEP_REEXEC_READY_PATH` | `try: unset DEADRECKON_SLEEP_REEXEC_READY_PATH` |
| `terminal width below 40 cols` | `try: pass --plain or widen the terminal` |
| `--prevent-sleep on requested but platform unsupported` | `try: --prevent-sleep off (Windows native prevention is a V1 candidate)` |
| `git commit hung waiting for gpg` (pre-P7 regression) | `try: rerun (P7 disables signing for commit-family verbs)` |

Each pair is exercised by a depth test in the matching phase (see
P5 / P6 / P7 / P9). The `parameterized over a depth test` invariant
is enforced by
`every_refusal_footer_ends_with_try_line` in P9.

## Config additions

```toml
[defaults]
prevent_sleep = "auto"  # "auto" | "on" | "off"
plain = false           # global default for --plain across verbs
```

`deadreckon config set defaults.prevent_sleep on` works through the
existing `config get/set` plumbing.

## Out of scope (V1 candidates — log if surfaced)

- **Windows native sleep prevention** (`SetThreadExecutionState`).
- **Tiered failure handling with backoff** (agent-reported / retryable
  / permanent / commit-failure 3-strike rule).
- **`--stop-when "<natural-language>"`** persistent stop condition.
- **Notes-buffer memory between turns** (agent-authored running notes
  fed back into the next-turn prompt).
- **Conventional-commit preset** for per-turn worktree commits.
- **Permanent-error stderr classification** for CLI providers (e.g.
  "credit balance is too low").
- **Companion-mode host SKILL.md** teaching Claude Code / Codex CLI
  how to delegate to deadreckon.
- **Terminal-title live updates** (mid-run elapsed/spend in the tab
  title).
- **Anonymous telemetry.**

If any phase surfaces one of these, log to
`/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` and continue. Do not
silently expand scope.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (utility, free):
- `crossterm` — already in the workspace for ratatui; gains
  `terminal::size()` callers in `ui_card`. No new dep.
- `nix` — already in the workspace for PID liveness; gains `kill`
  calls in `sleep::mac` for caffeinate reap. No new dep.
- `tempfile` — already in the workspace for tests; used by Linux
  `mkdtemp` of `deadreckon-sleep-*`. No new dep.

Tier 2 (architectural, log to `DEPENDENCIES.md`): none expected.

Tier 3 (blocked): same blocks as prior riders — no new HTTP, async,
sandbox, or terminal libraries.

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes.** Sleep state in files; spend
  flags derived from `spend.jsonl` aggregation.
- **One depth test before each phase implementation.** A phase whose
  tests were never red is suspect.
- **All user-facing rendering goes through `ui_card`.** No phase may
  add a freestanding `println!` / `writeln!` for surface content
  outside the renderer. A depth test in P9 greps for surface-formatting
  calls in changed files outside `ui_card.rs` / `cards/`.
- **Every commit-family `git` invocation in `deadreckon-core` and
  `deadreckon` routes through `run_git`** (P7). A depth test greps for
  raw `Command::new("git")` and asserts the count outside `git.rs`
  is zero.
- **Sleep prevention is reaped on every run-loop exit path**
  (completion, pause-at-cap, killed, failed, panic). The
  `SleepHandle` is RAII-bound to the run loop's lifetime, never to a
  phase outcome.
- **Linux ready-path is trusted-path-validated on both sides.** Both
  the parent's path-writing and the child's path-recognizing logic
  reject untrusted paths.
- **No silent expansion.** Anything beyond P1–P11 goes into
  `docs/V1-CANDIDATES.md`.
- **Spec pinning: card fixtures are golden.** P2/P3 each commit a
  byte-compared fixture under
  `crates/deadreckon/tests/fixtures/cards/` (ANSI-stripped).
  Whitespace changes are spec changes.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with the relevant depth tests passing and a
  one-line CHANGELOG entry naming the SHA after P11.
- After P11, optionally capture an asciinema cast under
  `/Users/gdc/deadreckon/demo-overnight.cast` showing the preview →
  caffeinate spawn → exit card flow.
- If a phase reveals a V1-architecture decision (Windows native
  prevention, GPU-backed display surfaces, terminal-emulator
  capability probing, etc.), stop and log in
  `docs/V1-CANDIDATES.md`; do not silently expand scope.
