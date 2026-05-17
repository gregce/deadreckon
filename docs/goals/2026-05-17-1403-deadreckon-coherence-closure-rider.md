# deadreckon — Coherence Closure Rider

This rider holds the implementation constraints for
`/Users/gdc/deadreckon/docs/goals/2026-05-17-1403-deadreckon-coherence-closure-goal.md`.
It supersedes the stale untracked coherence plan from 2026-05-13. It does not
supersede orchestration, plan-events, semantic-merge-repair, provider, chain, or
hygiene riders; those invariants still apply.

All paths are absolute. Source root: `/Users/gdc/deadreckon/`.

## Non-negotiables

- Keep maturity `alpha`.
- Do not remove working aliases unless a phase explicitly proves the alias is harmful. Prefer: canonical examples first, aliases inline and secondary, hidden compatibility aliases kept quiet.
- Preserve the visual identity:
  - cyan `deadreckoning`
  - course strip characters `* ^ . -`
  - magenta ids
  - spend gauge gradient
  - step glyphs `○ ● ◐ ✗ ↷ ◉ ↶`
- Do not add preview cards to list/status/history/show by default. Cards are allowed for preflight and completion summaries where they make a state transition easier to scan.
- No schema churn for durable run/plan/chain state unless strictly necessary. If needed, keep additions backward-compatible.
- No `git push`.
- Update `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` for anything larger than this closure, such as localization, theming, or a full template engine.

## Canonical user-facing words

Use these unless a specific advanced/debug context requires more detail.

| Concept | Canonical user word | Secondary/internal words |
|---|---|---|
| Single long task | run | pipeline only in code |
| Multi-agent object | plan | orchestration as category; job avoided |
| Plan execution action | orchestrate | plan/fork/merge for explicit advanced steps |
| Plan unit | child | task id when showing `task-0`; worker spec for files |
| Active status | running | stored `executing`/`forked` hidden behind glossary |
| Complete status | completed | stored `merged` hidden behind glossary |
| Done contract | done criteria | acceptance/gate in technical detail rows only |
| Main completion router | finish | direct apply/export after finish |
| Merge into source git | apply | keep alias secondary |
| Copy artifact out | export | materialize/copy-out secondary or hidden |
| Remove temp work | cleanup | direct abandon/discard advanced only |
| Inspect next action | status | next alias secondary |
| Detailed inspect | show | inspect alias secondary |
| Live view | attach | watch alias secondary |
| Cancel | kill | stop alias secondary |
| Continue incomplete | resume | continue alias secondary |
| Continue completed | extend | follow-up alias secondary |

Provider terms:

- Normal output: `provider`, `model`.
- Role output: `planner`, `coder`, `reviewer`, `child`, `doc provider`.
- Advanced provider docs may say `route` and `descriptor`, but command help should explain those terms once.

Flag terms:

- `--yes`: skip preflight/start confirmation.
- `--no-confirm`: skip destructive/completion confirmation.
- `--all`: all objects in the current object family.
- `--all-scopes`: all project scopes.
- `--plain`: no TUI/spinner/ANSI; line-oriented text.
- `--quiet`: suppress success chatter, not requested data or errors.
- `--json`: machine output; no ANSI, hints, prompts, or extra text.
- Strategy flags must be scoped: merge strategy, git apply strategy, chain apply mode.

## Stream policy

- stdout: requested data, success summaries, prompts, lifecycle hints after success.
- stderr: progress/spinners/wait lines, previews that are not requested data, warnings, errors, error hints.
- JSON mode: stdout only for the JSON document; stderr only for fatal diagnostics before JSON can be produced.
- TUI alternate screen: no extra stdout chatter while active.
- Cancellation text: stdout when the user declines a normal prompt; stderr when cancellation is an error path.

## Phase P1 — Freeze the matrix into tests

Depth tests first:

- Add or update snapshot-style tests for:
  - custom top help
  - help-all
  - `run --help`
  - `orchestrate --help`
  - `orchestrate review --help`
  - `orchestrate full-plan --help`
  - `plan --help`, `fork --help`, `merge --help`
  - `chain --help`
  - `finish --help`, `apply --help`, direct export/materialize help
  - `def-done --help`, `status --help`, `attach --help`, `kill --help`
- The first version should expose the current drift from `/Users/gdc/deadreckon/docs/design/USER-FACING-MATRIX.md`.

Implementation:

- Keep the tests narrow enough that wording is intentional, not brittle noise.
- Use helper render functions rather than shelling out if the codebase already has parser/render access.

## Phase P2 — One glossary for public words

Depth tests first:

- `RunStatus::Executing`, `PhaseStatus::Executing`, `PlanStatus::Forked`, and `PlanStatus::Merged` render as canonical labels.
- Public lifecycle action labels return `finish`, `apply`, `export`, `cleanup`, `status`, `attach`, `kill`, `resume`, `extend`.
- Provider role labels render consistently for full-plan and review modes.

Implementation:

- Extend `crates/deadreckon-core/src/glossary.rs` only for stable cross-crate state labels.
- Put CLI-only terms in a CLI glossary module if they do not belong in core.
- Replace local string matches for statuses and common action labels with helpers.
- Do not rename serialized enum variants in this phase.

## Phase P3 — One command/help table

Depth tests first:

- Top help and help-all include the same canonical command rows where scopes overlap.
- Aliases appear inline, not as separate lifecycle items.
- Examples use canonical commands first.
- `MERGE_HELP` teaches `finish <plan-id>` before any direct action.

Implementation:

- Create a small data table for command name, aliases, category, visibility, one-line purpose, and canonical examples.
- Feed `print_top_help`, `print_help_all`, and help constants from the same source where practical.
- If clap `after_help` cannot share a renderer cleanly, keep constants but add tests that compare them to the table.
- Remove "job" from plan-facing help unless in a generic sentence.

## Phase P4 — Flag policy helper

Depth tests first:

- Help text for `--plain`, `--quiet`, `--json`, `--yes`, `--no-confirm`, `--all`, and `--all-scopes` matches the policy in this rider.
- JSON mode suppresses hints and ANSI on all commands that support JSON.
- `--all` and `--all-scopes` examples no longer teach the wrong scope.

Implementation:

- Add shared constants or helper functions for repeated flag help.
- Make provider role flags read as a family in orchestrate/plan/fork help.
- Keep hidden aliases only where needed for compatibility; do not promote hidden aliases in examples.

## Phase P5 — One style, palette, and text-block facade

Depth tests first:

- Known statuses map to expected tones.
- `hint:` and `try:` lines render through one helper.
- Key-value blocks align consistently.
- `NO_COLOR`, `TERM=dumb`, `--plain`, and non-TTY behavior are tested for representative surfaces.

Implementation:

- Move public style wrappers from `main.rs` into `ui.rs` or one `output` module.
- Add helpers for `status_tone`, `status_text`, `hint`, `try_line`, `next_action`, `kv_block`, and lifecycle command rows.
- Centralize progress banner/course strip construction and test the `* ^ . -` output.
- Keep direct ratatui colors only behind palette roles where practical.

## Phase P6 — One prompt and preflight builder

Depth tests first:

- Confirm prompts show the right default marker and route to the right stream.
- Orchestrate interactive preflight can present:
  - review vs full-plan
  - child count
  - planner/coder/reviewer/default child providers
  - per-child overrides
  - repair on/off
  - spend and wall caps
  - source git/init-git/copy fallback
  - done criteria status
- Declining a normal preflight prints one consistent cancellation line.

Implementation:

- Build a reusable prompt/preflight helper that can be used by run, orchestrate, chain, update, finish/apply/export, and doc polish where appropriate.
- Do not over-abstract the business logic; centralize only prompt shape, default markers, stream, and action labels.

## Phase P7 — Lifecycle summary parity

Depth tests first:

- `run` started/completed, `extend`, `resume`, `orchestrate` started/completed, `plan` created, `merge` completed, `finish`, `apply`, `export`, `kill`, and `cleanup` outputs share:
  - object kind
  - primary id
  - status
  - provider/caps/source/gate where relevant
  - `finish <id>` first after completion
  - direct next actions second
- Plan id is primary in plan flows; child/result run ids are secondary detail.

Implementation:

- Introduce a lifecycle summary builder or small family of builders.
- Keep list/status/history as plain tables/text, not preview cards.
- Make `latest` wording explicit for run vs plan resolution.

## Phase P8 — Orchestration word and navigation cleanup

Depth tests first:

- `attach <plan-id>` footer uses the same grammar as run/chain where possible.
- Plan -> child -> back footer/breadcrumb text is visible and tested.
- `show <plan-id>` help and output explain child refs, result run, repair state, and next actions.
- Kill output for a plan reports child cascade count consistently.

Implementation:

- Update plan attach footer, plan summary, merge completion, show, status/list rows, and kill text.
- Show ready/blocked/parallel semantics in preflight: which children can run now, which wait on dependencies.
- Keep file-backed plan event polling as alpha; document broadcast bus as V1 if not already recorded.

## Phase P9 — JSON/plain/quiet contract

Depth tests first:

- JSON outputs contain no ANSI, no hints, and no human-only lines.
- Plain outputs contain no ANSI/TUI control and include the same essential ids/next actions.
- Quiet suppresses success chatter but not requested data.
- Representative JSON objects include `kind`, `id`, `status`, `next_actions` or `try_lines`, and relevant paths.

Implementation:

- Normalize JSON shapes without breaking existing essential fields.
- Add JSON for plan-like inspect/preview surfaces if the matrix finding remains relevant.
- Document any commands that intentionally do not support JSON.

## Phase P10 — Docs sweep

Depth tests first:

- Source/documentation scan for stale primary examples:
  - `deadreckon next` outside alias notes
  - `deadreckon materialize` as primary user example
  - `deadreckon discard` as primary user example
  - `orchestration jobs`
  - `deadreckon done`
- The scan should allow compatibility notes and historical changelog entries.

Implementation:

- Update README, HOWTO, AS-BUILT §§17/18/26/30/32, and relevant docs/goals references where they describe current user behavior.
- Update `/Users/gdc/deadreckon/docs/design/USER-FACING-MATRIX.md` to mark fixed/deferred findings.
- Update CHANGELOG with "Coherence closure (alpha)".

## Phase P11 — Full verification and final audit

Run focused tests after each phase. At milestone boundaries and before the final commit, run when practical:

```sh
cargo build --release
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

Final manual smoke:

- `deadreckon --help`
- `deadreckon help-all`
- `deadreckon run --help`
- `deadreckon orchestrate --help`
- `deadreckon orchestrate review --help`
- `deadreckon orchestrate full-plan --help`
- `deadreckon plan --help`
- `deadreckon merge --help`
- `deadreckon chain --help`
- `deadreckon finish --help`
- `deadreckon status --help`
- `deadreckon def-done --help`

Then run the stale-word scan from P10 and inspect the output manually.

Completion criteria:

- Matrix findings are fixed or explicitly deferred.
- Tests and snapshots prove the user-facing contract.
- AS-BUILT, CHANGELOG, and docs examples reflect current behavior.
- V1 deferrals are recorded.
- Local conventional commit created.
