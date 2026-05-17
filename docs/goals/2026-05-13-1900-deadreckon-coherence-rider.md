# deadreckon - Coherence Pass Rider

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-13-1900-deadreckon-coherence-goal.md`.
It supersedes nothing in prior riders. The audit at
`/Users/gdc/deadreckon/docs/design/USER-FACING-MATRIX.md` (108 numbered
findings) is the spec; this rider sequences fixes into eleven phases
with depth tests first.

The work is one editorial pass over user-visible surfaces. Internal
state, persistence, and provider plumbing stay as they are.

**All paths absolute.** Source `/Users/gdc/deadreckon/`. Runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided, do not redesign)

- **Maturity stays `alpha`.**
- **No `PipelineState` schema changes.** No new fields on `RunStatus`,
  `ChainStatus`, `ChainStepStatus`, `PhaseStatus`, `PlanTaskStatus`.
  Variant names stay. The *displayed* string changes via one helper.
- **One glossary file.** Status words, object nouns, action verbs all
  resolve through `crates/deadreckon-core/src/glossary.rs`. Any user
  string the binary prints reads from there or is a non-glossary
  literal (paths, ids, errors quoted verbatim from the OS).
- **One style module.** `crates/deadreckon/src/ui.rs` exposes `Tone`,
  `Stream`, `write`. Every other file in `crates/deadreckon/` either
  imports `ui::write` or prints structural plumbing (clap help, raw
  JSON to stdout for `--json`). Direct `\x1b[` escapes outside
  `ui.rs` fail the grep depth test.
- **One key/value block.** `print_kv_block(stream, tone, items)` is
  the only key/value formatter. The five sites at
  `main.rs:9661, 9773, 9338, 2787, 3015` collapse to one call shape.
- **One prompt builder.** `prompt::confirm(question, default)` plus
  `prompt::open(question, default)`. The eight Y/n + y/N sites all
  route through it.
- **No silent flag rename.** `--force`/`--all`/`--budget-cap`/`--branch`
  changes ship with aliases for one alpha; help text names the new
  flag first; deprecation logged in `CHANGELOG.md`.
- **Editorial bar.** `/Users/gdc/impeccable/STYLE.md` governs every
  user-visible string this rider touches. No em dashes. No "robust",
  "seamless", "elevate", "empower", "underscore", "pivotal", "delve",
  "moreover", "furthermore". Verbs lead. Numbers cited.
- **Visual identity preserved.** The following affordances are part
  of the product's personality. They survive every refactor below.
  - The `deadreckoning` cyan-bold word on the run-TUI footer status
    line (`main.rs:12328`).
  - The `* ^ . -` ASCII course strip rendered by
    `deadreckoning_course_ascii` (`main.rs:6633-6652`), used at full
    width in the run-TUI footer and at width 18 in
    `cli_wait_status_line` (`6622-6631`). Cadence: 200 ms per tick
    (run TUI), kept identical for chain and plan TUIs after this
    rider (T9 fix).
  - The `with_cli_wait_status` spinner that wraps non-TUI provider
    waits (`8255-8290`), used at `1781`, `4364`, `4658`, `6937`.
    Labels stay free-form but pass through the new style helper.
  - Magenta-bold run / chain / plan / provider IDs
    (`ui_id` -> `Tone::Id`).
  - The spend-gauge gradient: green &lt; 0.6, yellow &lt; 0.8, red &gt;= 0.8,
    magenta on `pause_reason == "spend cap reached"`
    (`meter_color` at `13072-13078`). Add a one-line legend in the
    TUI status line so the magenta state is no longer a magic string
    (T10 fix).
  - The Unicode chain-step glyphs `○ ● ◐ ✗ ↷ ◉ ↶`
    (`chain_step_dot` at `3961-3971`). P8 changes `Applied` from
    `●` to `◉` so it stops colliding with `Running`; everything else
    survives.
  - The acceptance result glyphs `✓ ✗ !` (`acceptance_result_line`
    at `10825-10834`). `✓` and `!` keep current colours; required-
    failure `✗` becomes `Tone::Negative` (red) to match the doctor
    failure semantics (C3 fix).
  - The markdown rendering palette in the docs panel (H1 cyan-bold,
    H2 light-cyan, code-fence dark-gray, link blue + underline). Only
    inline code colour changes (T11): inline `` `code` `` shifts
    from bold-yellow to light-green so it stops colliding with the
    yellow context-warning band. Code blocks already use light-green.
  - The "next:", "try:", "fix:", "hint:" prose markers. They keep
    their bold-blue / dim styling; only the call sites migrate.
  - Goldens in `crates/deadreckon/tests/coherence_visual.rs` capture
    a representative run-TUI frame, a chain-TUI frame, the new
    plan-attach frame, the `cli_wait_status` strip, and the run
    completion banner. Bytes are pinned. Any diff fails the build.
- **No `git push`.** Phased local commits.
- **No V1 invention.** Mass renames of `RunStatus` variants, multi
  locale support, themable palettes, full migration of `print_status`
  to a templating engine: all V1.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Audit anchor

Every fix below cites the audit ID (S1-S4, V1-V17, F1-F15, C1-C11,
O1-O17, T1-T14, L1-L3, P1-P24, Q1-Q5, R1-R15, CH1-CH6, J1-J3, D1-D2)
from `docs/design/USER-FACING-MATRIX.md`. When the matrix and the
rider disagree, the matrix wins; update the rider in the same commit.

## Orchestration coverage (post-audit additions)

The audit was taken at commit `455b91a`. The thirteen unpushed
commits on top of that introduce a multi-agent orchestration surface
(verbs `orchestrate`, `plan`, `fork`, `merge`, `history`; a third
TUI surface `render_plan_attach` at `main.rs:12225`; helper printers
`print_plan_created` `7049`, `print_plan_summary` `8082`; a
`PlanTaskStatus` enum mapped from `RunStatus` by
`plan_status_from_run_status` at `7637`). These are not in the
audit matrix yet, but they MUST land into the same coherence model.
This rider extends the audit findings to them.

### New verbs (catalogue)

| Verb | About | Heading | Aliases |
|---|---|---|---|
| `orchestrate <goal>` | Plan, fork, and merge a multi-agent run | Run Lifecycle | none |
| `plan <goal>` | Write an orchestration plan without starting children | Run Lifecycle | none |
| `fork <plan-id>` | Start child runs for an orchestration plan | Run Lifecycle | none |
| `merge <plan-id>` | Compose completed plan children into one promoted artifact | Run Lifecycle | none |
| `history grep <pattern>` | Search durable trace/provenance JSONL | Inspect And Import | none |
| `show <run-id> --why-failed` | New flag mirroring `chain show --why-failed` | (existing Show) | (existing) |

### Three-way `merge` collision

The string `merge` now means three different things:

| Surface | What `merge` is | Evidence |
|---|---|---|
| Top-level verb | Compose orchestration children into one artifact | `cli.rs:684` |
| `chain --branch-policy merge` | Branch policy enum value (rebase merge style) | `cli.rs:543` |
| `apply --strategy merge` | Git merge strategy for `apply` | `cli.rs:743` |

P5 resolves: keep the new verb `merge`. Rename `chain --branch-policy
merge` to `chain --branch-policy linear-merge` with `merge` kept as a
deprecated value alias for one alpha. Rename `apply --strategy` to
`apply --git-strategy` and keep `--strategy` as deprecated alias for
one alpha; the value `merge` is unchanged. Net effect: typing
`deadreckon merge ...` is always the orchestration verb; typing
`--git-strategy merge` is always the git operation.

### Two-way `plan` collision

`plan` is now a top-level verb AND a chain sub-verb
(`deadreckon chain plan "goal" --n 4`, see `CHAIN_HELP` at
`cli.rs:205`). The chain sub-verb is a planner that drafts steps;
the top-level verb is an orchestration plan writer. Both keep their
names; help text is updated to disambiguate. The chain sub-verb's
output line is reworded from `drafted: <chain-id>` to
`drafted chain steps: <chain-id>` to make the orchestration verb
unambiguous in shell history.

### New ID family

Plan IDs join run IDs and chain IDs. Same resolution shape:
`<plan-id> | <unique-prefix> | latest`. `resolve_plan_id` must use
the same refusal language as `resolve_chain_id` (ambiguous prefix +
candidate list, not-found + `try: deadreckon history grep`,
cross-scope rules).

### New status enum to align

`PlanTaskStatus` (see `plan_status_from_run_status` at `main.rs:7637`)
is rendered in `print_plan_summary`, `print_plan_created`, and the
plan-attach TUI. P1 routes its display through
`glossary::plan_task_status_label` so a child run that is
`Executing` appears as `running` in the plan attach view (matching
the rest of the binary). The on-disk variant name stays.

### New provider flags

Three new provider flag families introduced by orchestration:
`--planner-provider` (split mode), `--coder-provider` /
`--reviewer-provider` (review mode), and `--child-provider IDX=NAME`
(repeated per-child override on `orchestrate`/`plan`/`fork`). These
all describe the same concept as `--provider`/`--doc-provider` (a
provider route). P9 makes their help text use the anchor noun
"route". No rename, no alias change.

### `merge --strategy` and `--no-gate`

`merge` exposes `--strategy fail-on-conflict | prefer-child` and
`--prefer-child <idx>`. The flag is `--strategy` again, a third
semantic for that word (the audit already flagged `apply --strategy`
vs `chain --apply-strategy`). After P5 renames `apply` to
`--git-strategy`, the only remaining `--strategy` on a non-chain verb
is on `merge`; that one keeps the name. `merge --no-gate` is a
safety bypass; help text places it under "destructive options" and
the verb refuses with a confirmation prompt unless `--no-gate` is
explicit.

### `history grep` flag alignment

`history grep` ships with `--all`, `--scope`, `--plan`, `--since`,
`--kind`, `--limit`, `--regex` (`cli.rs:1165-1198`). After P5,
`--all` semantics on history match `list`/`library`/`providers list`
(cross-project). The flag does not rename. `--scope` matches `list`;
`--since` matches no other command and stays new (acceptable, it is
a search filter, not a state filter). `--plan` is novel (filter
children by parent plan); accept as is.

### Plan-attach TUI (third surface)

`render_plan_attach` at `main.rs:12225` joins `attach_tui` and
`chain_attach_tui` as the third ratatui surface. P8 puts it on the
shared `TuiPalette` and the shared `tui_footer` builder. Poll
cadence: 200 ms (match run TUI). Selection marker: `>` (match chain
TUI). Step glyph for a child run: same Unicode glyph set as
chain-step dots, mapping through `plan_task_status_label`. Header
key/value: `print_kv_block` shape.

### Polymorphic verbs (run-id vs plan-id dispatch)

Two verbs at HEAD silently dispatch on ID kind:

- `attach <id>` (`main.rs:11097-11130`): tries `load_cli_run`, falls
  back to `resolve_plan_id`, hands off to `attach_plan_tui` or
  `attach_tui`.
- `kill <id>` (`main.rs:11134-11187`): tries the run path, falls
  back to `kill_plan_command` at `11154`. The current plan-kill line
  reads `killed plan {id} ({n} processes signalled)`, which is a
  different shape from the run-kill `killed run {id}` /
  `killed run {id} forcefully`.

P7 routes both through `print_kill_banner(stream, kind, id, force)`
with `kind ∈ {Run, Chain, Plan}`. Output canon:
`killed run ab12cd34`, `killed run ab12cd34 forcefully`,
`killed chain ab12cd34`, `killed chain ab12cd34 forcefully`,
`killed plan ab12cd34 (3 processes signalled)`,
`killed plan ab12cd34 forcefully (3 processes signalled)`. The
trailing `(N processes signalled)` is plan-only and stays because
the user cares about the fan-out count.

P8 adds a one-line disambiguation banner to `attach`. Before the TUI
clears the screen, print `attaching to <kind> <prefix>` to stderr so
a user piping output knows whether they handed in a run id or a
plan id. `kind ∈ {run, chain, plan}` resolved by the dispatcher.

Depth tests (in `crates/deadreckon/tests/coherence_polymorphic.rs`):

- `attach_prints_kind_banner_before_tui_clears`
- `kill_banner_run_chain_plan_share_format_minus_processes_count`
- `kill_plan_force_adds_forcefully_before_processes_count`

### `--why-failed` parity (Show and Chain Show)

`show --why-failed` (added in commit 8865f1e, `cli.rs:489`) and
`chain show --why-failed` (`cli.rs:803`) currently produce different
prose. P4 routes both through one
`render_why_failed(stream, kind, reason, evidence)` helper. Same
section header, same `reason:` / `evidence:` / `try:` shape,
identical Tone application.

Depth tests:

- `show_why_failed_and_chain_show_why_failed_share_layout`
- `why_failed_for_killed_run_includes_killed_by_user`

### Plan inspection surface

There is no `plan list` / `plan show` verb. The audit goal's smoke
"`plan show`, `attach <plan-id>`, `merge <plan-id>`" reads against
the actually-implemented surface: `attach <plan-id> --plain` runs
through `print_plan_summary` (`main.rs:8082`) which IS the plan-show
view. `history grep --plan <plan-id>` searches the plan's child
traces. No new verb is introduced by this rider. The orchestration
roadmap may add `plan list` / `plan show` in a future alpha; that
work is V1 and logged in `docs/V1-CANDIDATES.md`.

Depth test:

- `attach_plan_id_plain_runs_print_plan_summary`

### New error-footer pairs (in addition to §"Error-footer canonical pairs")

| Error body | `try:` |
|---|---|
| `no plan '<id>'` | `deadreckon plan list` (or, when added, `deadreckon history grep <id>`) |
| `plan <id> has no completed children` | `deadreckon attach <id>` |
| `plan <id> child <idx> is <status>; merge requires all children completed` | `deadreckon attach <id>` |
| `merge produced conflicts; --strategy prefer-child --prefer-child <idx> selects one` | (verbatim, no try) |
| `history pattern empty` | `deadreckon history grep "<keyword>"` |
| `regex compile failed: <err>` | `deadreckon history grep "<literal>"` |

### Orchestration phase fold-in

These items merge into existing phases rather than adding P12:

- P1: extend `glossary` with `plan_task_status_label`,
  `NOUN_PLAN`, `NOUN_CHILD`.
- P3: add `PlanTaskStatus` and `merge --strategy` enums to the Tone
  lookup table.
- P4: include `orchestrate`/`plan`/`fork`/`merge`/`history` in
  top-level help and `print_help_all` parity. Disambiguate
  `plan` vs `chain plan` in `RUN_HELP` and `CHAIN_HELP`.
- P5: triple-`merge` rename, `--git-strategy` rename, the new
  `--planner-provider` / `--coder-provider` / `--reviewer-provider` /
  `--child-provider` flags audited for help-text consistency.
- P7: route `print_plan_created` and `print_plan_summary` through
  `print_kv_block`; route plan completion through
  `print_completion_banner` with `BannerKind::Plan`.
- P8: plan-attach TUI joins the shared palette + footer + 200 ms
  cadence. Goldens locked.
- P9: `--planner-provider` / `--coder-provider` / `--reviewer-provider`
  help strings use "route" anchor.
- P10: `--json` added to `plan list`, `plan show`, `history grep`.

## Data model (files, not fields)

No persistent state added. Two new source files:

### `crates/deadreckon-core/src/glossary.rs`

```rust
// Status words shown to users. Match arms are exhaustive over the
// existing on-disk enums.
pub fn run_status_label(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Pending   => "pending",
        RunStatus::Planned   => "planned",
        RunStatus::Executing => "running",   // S1: align with chain
        RunStatus::Completed => "completed",
        RunStatus::Failed    => "failed",
        RunStatus::Killed    => "killed",
    }
}

pub fn chain_status_label(s: ChainStatus) -> &'static str { /* matches run words */ }
pub fn step_status_label(s: ChainStepStatus) -> &'static str { /* parallel */ }
pub fn phase_status_label(s: PhaseStatus) -> &'static str { /* parallel */ }
pub fn plan_task_status_label(s: PlanTaskStatus) -> &'static str {
    // Plan children render with the same words as runs.
    // Plumbs through plan_status_from_run_status at main.rs:7637.
}

// Action labels for completion banners.
pub fn run_action_label(outcome: RunLoopOutcome) -> (&'static str, Tone) {
    match outcome {
        Done        => ("completed", Tone::Ok),
        PausedAtCap => ("paused",    Tone::Warn),
        Killed      => ("killed",    Tone::Negative),
        Failed      => ("failed",    Tone::Negative),
    }
}

// Chain event labels: all verb-first, all phrases.
pub fn chain_event_label(e: ChainEventKind) -> &'static str { /* S2-S3 fix */ }

// Object nouns. Use the constants below in every help string and
// every refusal message. Grep depth test forbids "task" outside test
// fixtures.
pub const NOUN_RUN: &str = "run";
pub const NOUN_CHAIN: &str = "chain";
pub const NOUN_STEP: &str = "step";
pub const NOUN_TURN: &str = "turn";
pub const NOUN_PROVIDER: &str = "provider";
pub const NOUN_ROUTE: &str = "route";
pub const NOUN_MODEL: &str = "model";
pub const NOUN_PLAN: &str = "plan";    // orchestration object
pub const NOUN_CHILD: &str = "child";  // plan child run
```

### `crates/deadreckon/src/ui.rs`

```rust
pub enum Stream { Stdout, Stderr }

pub enum Tone {
    Heading,   // bold cyan,   1;36
    Muted,     // dim,          2
    Id,        // bold magenta, 1;35
    Command,   // bold blue,    1;34
    Ok,        // bold green,   1;32
    Warn,      // bold yellow,  1;33  (mid-band caution)
    Negative,  // bold red,     1;31  (failure on a non-error stream)
    Hint,      // bold blue,    1;34, prefixed "hint: "
    Error,     // bold red,     1;31, stderr-only
}

pub fn write(stream: Stream, tone: Tone, text: &str) -> String;
pub fn writeln(stream: Stream, tone: Tone, text: &str);
pub fn kv_block(stream: Stream, items: &[(&str, String)]);
pub fn hint(stream: Stream, body: &str);     // routes through Tone::Hint
pub fn confirm(question: &str, default: bool) -> Result<bool>;
pub fn open(question: &str, default: Option<&str>) -> Result<String>;
```

The eight legacy `ui_*` helpers in `main.rs:130-205` become thin
wrappers that delegate to `ui::write`, then their call sites migrate
phase by phase. `ui_error_stdout` (`main.rs:4804`) is deleted.

## Mode resolution (decided)

Status word displayed = `glossary::run_status_label(state.status)`.
The audit-trail string written to disk stays the on-disk Display
impl in `state.rs:30-42`. Two functions; the second is for JSON, the
first is for humans.

```rust
fn human(s: RunStatus) -> &'static str { glossary::run_status_label(s) }
fn machine(s: RunStatus) -> String     { s.to_string() } // serde kebab-case
```

## Verb signatures

```
deadreckon kill <run-id> [--escalate]    # --force kept as alias one alpha
deadreckon chain kill <chain-id> [--escalate]
deadreckon abandon <run-id> [--anyway]   # was --force on abandon
deadreckon cleanup [--all-scopes] [--escalate] [--anyway]
deadreckon chain run [--all-scopes]
deadreckon list [--all]                  # --all kept: cross-project
deadreckon library list [--all]          # --all kept: cross-project
deadreckon status [--global]             # was --all
deadreckon providers list [--all]        # --all kept: include-uninstalled
deadreckon run "<goal>" [--branch-name <name>]
deadreckon apply <run-id> [--into <branch>] [--overwrite]
deadreckon finish <run-id> [--into <branch>] [--overwrite]
deadreckon materialize <run-id> [--overwrite]
deadreckon doc <run-id> --polish [--max-spend <usd>]   # was --budget-cap
```

Refusal cases (use `glossary::NOUN_*` for nouns):

| Verb | Trigger | Refusal | `try:` |
|---|---|---|---|
| `apply` | not Completed | `run <id> is <status>` | `deadreckon resume <id>` (Failed) / `deadreckon attach <id>` (Executing) |
| `apply` | not Worktree | `run <id> mode is <mode>; apply is for worktree runs` | `deadreckon materialize <id> --dest <path>` |
| `apply` | non-interactive | `non-interactive apply requires --no-confirm` | `deadreckon apply <id> --no-confirm` |
| `abandon` | Executing without `--anyway` | `run <id> is running` | `deadreckon kill <id> --escalate` |
| `extend` | InPlace parent | `extend is not available for in-place runs` | `deadreckon run --in-place --i-know-its-a-lot "<goal>"` stays out; replace with `deadreckon doc <id>` (see R11) |
| `cleanup <id>` | Executing without `--escalate` | `run <id> is running` | `deadreckon kill <id>` |
| `chain pause` | not Running | `chain <id> is already <status>` | `deadreckon chain status <id>` |
| `chain extend` | Completed without `--insert-at` | `cannot append to a completed chain` | `deadreckon chain extend <id> --insert-at <n>` |
| `doc --polish` | not Completed | `run <id> is <status>; docs polish requires a completed run` | `deadreckon resume <id>` |
| `doc --polish` | no doc provider | `no doc provider configured` | `deadreckon config set defaults.doc_provider cli:codex` |

## Banners + key/value blocks (decided)

One layout. Always two-space indent, key right-padded to the widest
key, colon, two spaces, value. Used by every status-card-shaped
output.

```
run:        ab12cd34 (full-id)
status:     running
goal:       <truncated to 110>
provider:   cli:codex
sandbox:    sandbox-exec
spend:      $0.123456 / $10.00
wall:       42.0s / 600s
```

Completion banner uses one builder regardless of entry point (run,
extend worktree, extend copy, resume):

```rust
print_completion_banner(stream, outcome, kind: BannerKind::{Run, Extended, Resumed}, run_id)
// "completed run ab12cd34", "completed extended run ab12cd34",
// "resumed run ab12cd34 to completion" all share the colour rules in
// glossary::run_action_label.
```

## Phases (eleven)

Each phase: write the named depth test(s) first, watch them fail,
implement, run
`cargo nextest run --workspace && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings`,
green, conventional-commit local commit, append one-line CHANGELOG
entry.

### P1 - Glossary module + status_label single source of truth

- Add `crates/deadreckon-core/src/glossary.rs` as in Data Model.
- Re-export from `deadreckon_core::glossary`.
- Replace every literal status word in `crates/deadreckon/src/main.rs`
  with a call through the glossary. Search anchor: every `match s {
  RunStatus::… }` or string literal `"running"`, `"completed"`,
  `"executing"`, `"failed"`, `"killed"`, `"paused"`, `"applied"`,
  `"undone"`, `"skipped"`, `"pending"`, `"planned"` inside a display
  path (not on-disk Display).
- TUI footer (`main.rs:12335-12360`) flips `Completed` to display
  `completed` (S1, T1) by routing through `run_status_label`.
- Chain TUI poll cadence aligned to 200 ms (T9): change `2821` to
  `Duration::from_millis(200)`.

Depth tests (`crates/deadreckon/tests/coherence_glossary.rs`):

- `every_runstatus_variant_renders_through_glossary`
- `tui_footer_says_completed_not_complete`
- `chain_label_running_matches_run_label_running`
- `audit_no_literal_status_words_outside_glossary_and_serde`
- `chain_event_labels_all_verb_first`

### P2 - Style helper module + raw-ANSI ban

- Add `crates/deadreckon/src/ui.rs` with `Tone`, `Stream`,
  `write`/`writeln`/`kv_block`/`hint`/`confirm`/`open`.
- Delete `ui_error_stdout` (`main.rs:4804`); migrate the one call to
  `ui::writeln(Stream::Stderr, Tone::Error, ...)`.
- Migrate the six raw-ANSI sites at `main.rs:204`, `4805`,
  `6626-6627`, `7082`, `7218` to `ui::write`.
- Keep legacy `ui_heading`/`ui_muted`/`ui_id`/`ui_command`/`ui_ok`/
  `ui_warn`/`ui_prompt_prefix`/`ui_status`/`ui_error` as thin
  wrappers that forward; mark `#[deprecated(note = "use ui::write")]`.

Depth tests:

- `audit_no_raw_ansi_escapes_outside_ui_module` (`grep -nE
  '\\x1b\\[|"\\[1;3' src/main.rs | grep -v ui.rs` returns empty).
- `ui_hint_renders_dim_blue_with_two_space_indent`
- `ui_error_writes_only_to_stderr`
- `ui_status_with_status_label_for_running_is_warn_not_negative`

### P3 - Tone for status, retire ui_status string matching

- Replace `ui_status(text)` heuristic with
  `ui::status(state, RunStatus)` that picks a `Tone` from a fixed
  table:

  | Status | Tone |
  |---|---|
  | running, applied, completed, passed, polished, ok | Ok |
  | paused, planned, pending, configured | Warn |
  | failed, killed, undone | Negative |

- Update `print_provider_list_row` (`main.rs:1145`), doctor row
  glyphs (`main.rs:5984-6273`), provider probe `✗` (C3) to use
  `Tone::Negative` for failure and `Tone::Warn` for "missing but
  not fatal" cases.

Depth tests:

- `ui_status_running_is_ok_tone`
- `ui_status_failed_is_negative_tone`
- `doctor_failed_row_uses_negative_not_warn`
- `acceptance_required_failure_glyph_is_negative_and_optional_is_warn`

### P4 - Verb + alias hygiene

- Un-hide the canonicals the user is told to type via after-help:
  `apply`, `abandon`, `materialize`, `doc`, `show`, `library`, `undo`,
  `import`, `acceptance` (V1, V4).
- Stop using aliases in after-help bodies (`cli.rs:227-256`). Keep
  the alias declarations; just lead with the canonical name in prose.
- Add `chain`, `config`, `list`, `detect`, `providers` to
  `TOP_LEVEL_HELP` and `print_top_help` (V3).
- Add `detect`, `providers` to `print_help_all` (V4); move `config`
  to "Setup" bucket (V5).
- Align `kill`'s "Cancel a running task" copy: switch "task" to
  "run" everywhere ("task" remains banned by glossary depth test).
- Pick one heading style (Sentence case + colon, mirroring
  `print_top_help`); migrate `print_help_all` and clap
  `next_help_heading` values (V7).
- Resolve description drift between clap `about` and
  `print_help_all`: clap `about` is the source, `print_help_all`
  reads it (V8).

Depth tests:

- `top_level_help_mentions_every_visible_command`
- `help_all_mentions_every_command`
- `no_after_help_uses_alias_when_canonical_exists`
- `audit_no_task_noun_in_help_text`
- `clap_about_matches_help_all_descriptions`

### P5 - Flag truth (force, all, branch, max-spend)

- Split `--force`:
  - `Kill`/`ChainKill`: `--escalate` primary, `--force` deprecated alias.
  - `Finish`/`Materialize`/`Doc`: `--overwrite` primary, `--force`
    deprecated alias.
  - `Abandon`: `--anyway` primary, `--force` deprecated alias.
  - `Cleanup`: keeps `--escalate` semantics (kill stale) and adds
    `--overwrite` for git worktree force; `--force` deprecated.
- Split `--all`:
  - `List`, `Library`, `Providers list`: `--all` keeps current
    meaning.
  - `Cleanup`, `Chain`: rename to `--all-scopes`; `--all` deprecated
    alias.
  - `Status`: rename to `--global`; `--all` deprecated alias.
- Rename `Doc --budget-cap` to `--max-spend` (F7); keep alias.
- `Run --branch` becomes `--branch-name`; `Apply --branch` and
  `Finish --branch` become `--into`. Aliases kept (F4).
- Document `--no-confirm` semantics in each command's after-help.
- Help text drift fix: one phrase for `--no-hints` shared across
  Run/Chain/Attach (F9). Phrase: `Suppress post-action hints`.

Depth tests:

- `kill_force_alias_resolves_to_escalate`
- `materialize_force_alias_resolves_to_overwrite`
- `abandon_force_alias_resolves_to_anyway`
- `status_all_alias_resolves_to_global`
- `doc_budget_cap_alias_resolves_to_max_spend`
- `every_force_use_in_after_help_uses_new_name`
- `no_hints_help_text_is_identical_across_commands`

### P6 - Output stream policy + hint unification

- Centralise all user-visible printing through three free functions
  in `ui.rs`:
  - `out::info(text)` -> stdout, no tone.
  - `out::ok(text)` / `out::warn(text)` / `out::negative(text)` /
    `out::error(text)`. Last one is stderr-only.
  - `out::hint(text)` -> stderr, `Tone::Hint`, two-space indent,
    `hint:` prefix.
- Replace the three visual styles of `hint:` (`main.rs:1109, 204,
  7384, 7388, 8312, 10269, 8337, 8389, 8598, 8644`) with `out::hint`.
- Send `cancelled` after a stderr preview to stderr (O11).
- Chain-attach error paths (`main.rs:2849-2882`) route through
  `out::error` plus `error_hint` (O3).
- Replace hard-coded `/Users/gdc/.deadreckon/config.toml` in
  `error_hint` (`main.rs:121-123`, O1) with
  `paths.config_path().display()`.
- Replace hard-coded `/Users/gdc/deadreckon` in doctor source line
  (P5 audit, `main.rs:5949`, O5) with the discovered source root.
- Replace `DEFAULT_CONFIG_PATH` constant in
  `crates/deadreckon-providers/src/config.rs:8` with a path resolved
  at runtime (O4).
- Add a fallback hint for `Core(_)` / `Provider(_)` in `error_hint`
  (O2): `try: deadreckon doctor`.

Depth tests:

- `every_hint_uses_out_hint`
- `cancelled_lands_on_stderr_after_stderr_preview`
- `chain_attach_errors_print_styled_red_error_with_hint`
- `error_hint_uses_resolved_config_path_not_developer_home`
- `doctor_source_line_uses_runtime_root`

### P7 - `print_kv_block` and one banner builder

- Add `ui::kv_block(stream, tone, &[("run", id), ("status", ...), ...])`.
- Replace `print_status` (`main.rs:9661-9709`), `print_run_summary`
  (`9773-9779`), `show_command`'s mode/branch lines (`9338-9355`),
  `print_chain_attach_snapshot` policy line (`2787`),
  `chain_attach_header_text` policy line (`3015`), `chain_show_command`
  policy line (`2723`) with `kv_block` calls. All produce the same
  visual: lowercase keys, colon, right-padded.
- Spend precision is `${:.6}` everywhere (O14); chain snapshot
  upgrades from `:.2` (T7).
- Wall precision is `{:.1}s` everywhere (O15).
- Add `print_completion_banner(stream, outcome, kind, run_id)` and
  route `run` (`4283-4288`), inline extend (`7787-7792`), worktree
  extend (`7976-7979`), `resume` (`9275-9279`) through it (R1).
- `kill_command` and chain-kill output use one helper:
  `print_kill_banner(stream, kind, run_id, force)` so the
  `forcefully` suffix appears on chain kill too (R3, CH equiv).
- `materialize` (R5): success message becomes `materialized run
  <id>` (matches command name). Add a one-line note "(aliased as
  `export`/`copy-out`)" only when stdout is a tty and `--no-hints`
  is unset.
- `print_run_locations` (`9783-9792`) lines adopt `kv_block` shape:
  `state: <path>`, `launch-dir: <path>` (O13).

Depth tests:

- `print_kv_block_aligns_keys_to_widest`
- `status_card_and_run_summary_share_layout`
- `completion_banner_run_extend_resume_share_format`
- `kill_force_suffix_on_run_and_chain`
- `materialize_success_says_materialized`
- `spend_precision_six_decimals_everywhere_outside_tui_gauge`

### P8 - TUI alignment

- Build `TuiPalette` in `ui.rs` with named slots: `border_focused`,
  `border_idle`, `status_running`, `status_completed`, `status_failed`,
  `acceptance_default`, `acceptance_configured`,
  `acceptance_running`, `acceptance_passed`, `acceptance_failed`,
  `spend_low`, `spend_mid`, `spend_high`, `spend_pause_cap`. The Run
  TUI and Chain TUI both consume `TuiPalette` and stop hard-coding
  `ratatui::Color`.
- Split `acceptance_color`: `DefaultGate` becomes muted gray;
  `Configured` stays yellow (C8, T13).
- Chain step glyph for `Applied` changes from `●` to `◉` (C7, T3).
- Chain TUI gains focus indicator (`*` title prefix + cyan border)
  on the steps panel (T5, T12).
- Trace `latency_ms:?` becomes `latency_ms.map(|n| format!("{n}ms"))
  .unwrap_or_else(|| "-".into())` (T4, audit `main.rs:12567`).
- Process row collapses to `{pid} {state}` without re-printing
  state in the command column (T6, `main.rs:12925, 10867-10870`).
- One footer builder `tui_footer(state, focus_hint)` used by both
  TUIs; both call it; the snapshot footer for chain attach reuses
  the same builder (T2, T13).
- Run TUI in-progress and completion footers share bracket-key
  notation `[d] Docs  [a] Apply  q detach` and drop the
  `Detach: ... | Focus: ... | Scroll: ...` form (T2).

Depth tests:

- `tui_palette_has_named_slot_for_every_color_consumer`
- `chain_tui_focus_marker_renders_on_steps_panel`
- `chain_step_applied_glyph_differs_from_running`
- `trace_line_renders_latency_without_debug_braces`
- `process_row_does_not_duplicate_state`
- `tui_footer_run_and_chain_share_format`

### P9 - Provider terminology + doctor truth

- Pick anchor nouns: `provider` = vendor (`anthropic`, `openai`),
  `route` = transport id (`cli:codex`), `model` = model id,
  `descriptor` is internal only. Update every clap arg help string
  in `cli.rs` to use these terms once and consistently (P13).
- `format_provider_kind` is deleted. `print_provider_selection` reads
  `DescriptorKind` so `kind=` reads `cli`/`http`/`local-http`/
  `scripted` in `provider selection`, `providers list`, and
  `detect --json`, identically (P8).
- `print_provider_selection` shows a `*` for the active route AND
  `providers list` shows a `*` for the configured route. Same marker
  on both surfaces (P9).
- `doctor` reads `ProviderRegistry::with_overrides` instead of name
  heuristics (P7, audit `main.rs:6042-6098`); subscription-binary
  check runs only for providers actually configured (P6, audit
  `main.rs:6258-6279`).
- `doctor` "source" line reads the resolved repo root, not literal
  `/Users/gdc/deadreckon`.
- `doctor` skips sandbox checks that do not apply on the host OS
  (P19, `main.rs:5961-5987`): macOS skips `bwrap`/`docker` unless
  they are actually configured.
- `auto_subscription_cli_provider` returns `None` when no
  subscription is available; init falls back to `anthropic` only at
  the call site (P4, `main.rs:838`).
- `auto_subscription_cli_provider` ordering aligns with
  `prompt_provider`: PATH preference `claude` -> `codex` ->
  `anthropic` (P3).
- `init` refuses to overwrite an existing config without
  `--overwrite` (P1, `main.rs:823-868`).
- `init` validates `--provider` against the registry; unknown ids
  refuse with `try: deadreckon providers list --all` (P2).
- `config set` validates the key against a known-keys list (P15).
- `config` gains `show` (read-only dump) and `unset <key>` (P17).
- `DEFAULT_CONFIG_PATH` removed from
  `crates/deadreckon-providers/src/config.rs:8` (O4).

Depth tests:

- `provider_kind_token_same_on_selection_and_list_and_detect`
- `provider_active_marker_is_star_on_both_surfaces`
- `init_refuses_overwrite_without_flag`
- `init_rejects_unknown_provider_id`
- `doctor_skips_unconfigured_subscription_binaries`
- `doctor_source_line_resolves_repo_root`
- `config_set_unknown_key_refuses_with_try_hint`
- `config_show_prints_full_config_pretty`
- `auto_subscription_returns_none_when_no_path_match`

### P10 - Prompts, JSON, lifecycle hints policy

- `prompt::confirm("question", default_yes: bool)` is the only
  prompt builder. All eight Y/n + y/N sites route through it. Output:
  `? question [Y/n]: ` or `? question [y/N]: ` with trailing colon
  (Q1).
- Doc-polish prompt logic accepts Enter as the labelled default
  (Q2, `main.rs:8891-8895`).
- Spend-cap prompt becomes `? continue with --max-spend $50? [y/N]:`
  (Q1).
- Add `--json` to `list`, `chain list`, `providers list`, `library
  list`, `status`, `show`, `doctor` (J1, J2). JSON shape mirrors the
  existing `ProviderProbeResult` style: one top-level object with a
  named array. JSON `try_lines` always present.
- `status` only emits `print_lifecycle_hints` when the run is
  Completed (R6, `main.rs:9647`). For Running/Failed/Killed states,
  the `run health` block already shows `next:`.
- `--no-hints` is honoured by every hint-emitting command (`list`,
  `cleanup`, `chain pause`, `chain extend`, `chain redo`,
  `chain undo`, `resume`, `kill`). Add `--no-hints` flag where it is
  currently missing (F9 plus matrix R4).
- `resume` emits `print_run_locations` + `print_lifecycle_hints` on
  Completed outcomes, matching `run` (R4).

Depth tests:

- `every_prompt_routes_through_prompt_confirm`
- `doc_polish_enter_is_yes`
- `spend_cap_prompt_has_trailing_colon_and_lowercase_continue`
- `list_chain_list_providers_list_library_list_emit_valid_json`
- `status_omits_lifecycle_hints_for_running_runs`
- `resume_emits_locations_and_hints_like_run`
- `no_hints_flag_present_on_every_hint_emitting_command`

### P11 - AS-BUILT + CHANGELOG + V1 candidates (doc only)

- Insert a new top-level section into
  `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:

  ```
  ## 26. Coherence pass (alpha)

  26.1 Glossary as single source of truth for status, nouns, verbs.
  26.2 Style helpers and the ANSI ban outside `ui.rs`.
  26.3 `print_kv_block` and the unified key/value layout.
  26.4 Flag truth: `--force`/`--all`/`--branch`/`--max-spend`.
  26.5 Confirmation builder and prompt parity.
  26.6 TUI palette, focus indicator parity, chain step glyph fix.
  26.7 Provider terminology anchor: provider/route/model/kind.
  26.8 JSON parity for list/show/status surfaces.
  26.9 Hint policy and `--no-hints` coverage.
  ```

- In the "what's shipped vs thin" section: remove "surfaces drift"
  from the thin list; add "coherence pass" to shipped. Note that
  mass renames of stored enum variants and theming remain V1.
- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:

  ```
  ## Coherence pass (alpha) - 2026-05-13

  - One glossary for status words; `running` replaces `executing` in
    every user-visible surface.
  - One style module; raw ANSI codes outside `ui.rs` removed.
  - One key/value block; `print_status`, `print_run_summary`,
    `show_command`, chain show/attach share layout.
  - Flag truth: `--force`/`--all`/`--branch`/`--budget-cap`
    deprecated with aliases for one alpha; `--escalate`,
    `--overwrite`, `--anyway`, `--all-scopes`, `--global`,
    `--branch-name`, `--into`, `--max-spend` standard.
  - JSON parity on list-shaped commands.
  - Doc-polish prompt label and logic now agree.
  ```

- Append to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`:
  - Mass rename of `RunStatus::Executing` to `RunStatus::Running`
    (on-disk schema change).
  - Themable palettes via config.
  - Localisation hooks for status words.
  - Migration from hand-built status cards to a templating engine.

## Integration matrix

| Concern | Source of truth | Consumer |
|---|---|---|
| Status word | `glossary::*_status_label` | `print_status`, `print_run_summary`, `show_command`, TUI footer, JSON-mode `human` field |
| Tone | `ui::Tone` | `ui::write`, `print_completion_banner`, `print_kill_banner` |
| Key/value | `ui::kv_block` | every card-shaped output |
| Footer text | `tui_footer` | Run TUI, Chain TUI, chain snapshot |
| Palette | `ui::TuiPalette` | Run TUI, Chain TUI |
| Confirm | `prompt::confirm` | every Y/n + y/N site |
| Hint | `out::hint` | every `hint:` site |
| Error label | `out::error` | every error printer including chain-attach |
| Kind token | `DescriptorKind` | provider selection, providers list, detect (JSON + human) |
| `--no-hints` | `completion_hints_enabled` | every hint-emitting command |

## Error-footer canonical pairs

| Error body | `try:` |
|---|---|
| `run <id> is <status>; <verb> requires a completed run` | `deadreckon resume <id>` (Failed/Killed) or `deadreckon attach <id>` (Executing) |
| `run <id> mode is <mode>; <verb> is for <mode2> runs` | `deadreckon <verb-for-mode> <id> --dest <path>` |
| `non-interactive <verb> requires --no-confirm` | `deadreckon <verb> <id> --no-confirm` |
| `chain <id> is already <status>` | `deadreckon chain status <id>` |
| `unknown provider id '<id>'` | `deadreckon providers list --all` |
| `config key '<key>' is not recognised` | `deadreckon config show` |
| `no provider configured` | `deadreckon init` |
| `home '<path>' is not writable` | `chmod u+w <path>` |
| `disk space low: <N> MB free in <path>` | `deadreckon cleanup --all-scopes` |

## Config additions

None. `--no-hints` env (`DEADRECKON_HINTS`) keeps current semantics.

## Out of scope

- Renaming on-disk `RunStatus::Executing` to `Running`. The display
  string changes; the variant name stays.
- Mass rewrites of help text past the cited drift sites. Drift fixed,
  voice not re-edited.
- Multi-locale support.
- A theming layer that lets users override the palette.
- Migrating status cards to a templating engine.
- Adding `--json` to every command. Only the cited list-shaped ones.
- New persistent state files.
- Provider transport changes.
- New providers, new descriptors, new ingest.
- New orchestration verbs or modes. `orchestrate`, `plan`, `fork`,
  `merge`, `history` ship as the unpushed commits left them; this
  rider only aligns vocabulary, colour, streams, and flags.
- Redesigning the `deadreckoning_course_ascii` strip, the spinner
  cadence, or the Unicode glyph alphabet. Any "but a Braille spinner
  would be cleaner" thought belongs in `docs/V1-CANDIDATES.md`.
- Replacing Unicode glyphs with ASCII for terminal-compat. The
  current renderer is the personality.
- New TUI surfaces. Three is enough; the goal aligns them.

## Dependencies

Tier 1 (utility, free): no new crates expected. Existing `ratatui`,
`crossterm`, `clap`, `serde`, `toml`, `console` cover the work.

Tier 2 (architectural, log to `DEPENDENCIES.md`): none expected.

Tier 3 (blocked): same as prior riders.

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes.** Status display changes only.
- **One depth test before each phase implementation.** A phase whose
  tests were never red is suspect.
- **No silent flag rename.** Every renamed flag ships with its old
  name as a deprecated alias for one alpha. Deprecation logged in
  `CHANGELOG.md`.
- **No string-match status colouring.** `ui::Tone` is the only thing
  that picks a colour for a status; the lookup table in P3 is the
  spec.
- **No raw ANSI outside `ui.rs`.** Grep depth test enforces.
- **Glossary forbids "task" as a noun for a run.** Grep depth test
  fails the build if "task" appears in user-visible copy outside
  fixtures.
- **No silent scope expansion.** Anything beyond P1-P11 goes into
  `docs/V1-CANDIDATES.md`.
- **Editorial bar.** Every user-visible string this rider edits must
  pass the `STYLE.md` denylist. Sweep with `grep -E
  '\b(robust|seamless|elevate|empower|underscore|pivotal|tapestry|delve|moreover|furthermore|load-bearing|highest-leverage|biggest unlock|reflex defaults|data-driven|in summary|in conclusion|let.s dive in|gone are the days|whether you.re)\b' crates/ docs/` and fail the build on hits inside user copy.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with the relevant depth tests passing and a
  CHANGELOG bullet naming the commit subject.
- After P11, capture an asciinema cast of `deadreckon status latest`
  on a completed run and on a failed run, plus a `chain attach`
  session, under `/Users/gdc/deadreckon/demos/coherence/`. Skip if
  the cast would not show a visible change.
- If a phase reveals a V1-architecture decision (mass enum rename,
  templating engine, theme system), stop and log it in
  `V1-CANDIDATES.md`. Do not silently expand scope.
