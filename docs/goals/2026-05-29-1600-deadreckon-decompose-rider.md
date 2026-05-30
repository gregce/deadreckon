# deadreckon — Decompose Rider (split the 40.6k-line main.rs behind a characterization net)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-29-1600-deadreckon-decompose-goal.md`.
It supersedes nothing in prior riders (notably
`/Users/gdc/deadreckon/docs/goals/2026-05-12-2039-deadreckon-hygiene-rider.md`,
`/Users/gdc/deadreckon/docs/goals/2026-05-11-1502-deadreckon-codebase-rider.md`,
`/Users/gdc/deadreckon/docs/goals/2026-05-28-1841-deadreckon-campaign-rider.md`,
`/Users/gdc/deadreckon/docs/goals/2026-05-29-1354-deadreckon-navigable-rider.md`) —
their invariants, dependency policy, files-not-fields pattern, and all shipped
behavior still apply. This rider is a **behavior-preserving refactor**: it does not
add a verb, a flag, a file, or a field. It decomposes the
`/Users/gdc/deadreckon/crates/deadreckon/src/main.rs` monolith into cohesive private
modules behind a thin dispatcher, lands a CLI-output characterization net **first** so
equivalence is provable, and finishes with a tight set of independent library
cleanups.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime `~/.deadreckon/`.

## The idea in one paragraph (read before designing)

`/Users/gdc/deadreckon/crates/deadreckon/src/main.rs` is **40,597 lines**: roughly
11k lines of command handlers and free-function helpers plus roughly 29k lines of
inline `#[cfg(test)]` modules (`acceptance_integrity_tests`,
`acceptance_render_tests`, `campaign_spawn_tests`, `effortless_consistency_tests`,
`flight_cli_tests`, `self_improve_pr_tests`, `tui_tests`), all in one file with a flat
crate-private namespace and no `commands/` structure. The dominant maintainability
pain is this single file. The crucial fact that makes decomposition *safe* is that the
**binary crate is NOT in the public-surface baseline** — `/Users/gdc/deadreckon/tests/.public-surface-baseline`
covers only `deadreckon-core`, `deadreckon-providers`, `deadreckon-runtime`, and
`deadreckon-sandbox`. So moving `main.rs` code into private `commands/` and `tui/`
modules is genuinely zero-public-surface-risk. The one prerequisite: the existing
integration tests (`/Users/gdc/deadreckon/crates/deadreckon/tests/orchestrate.rs`,
`/Users/gdc/deadreckon/crates/deadreckon/tests/chain.rs`) assert **state and file
existence, not user-facing stdout/stderr formatting** — so a CLI-output
characterization net must exist **before any move**, or render/format/error refactors
could silently change output. We land that net (P1–P2), lift the inline test modules
out of the source file (P3, shrinking it ~70% and surfacing exactly which symbols must
become `pub(crate)`), carve command families into private `commands/` modules behind a
thin `main_inner` dispatcher (P4–P5), extract the pure TUI render layer into `tui/`
(P6), unify the genuinely-duplicated merge and command-exists logic (P7–P8), and close
with cheap, independent library cleanups (P9–P10). Nothing about runtime behavior
changes; the work is **relocation plus the minimum visibility widening**, proven
byte-identical by snapshots captured first.

## Posture (decided — do not redesign)

- **Behavior-preserving refactor only.** Every command's stdout, stderr, exit codes,
  and file/state side effects stay **byte-identical**, proven by characterization
  snapshots captured before any move. No verb, flag, file, field, or output-string
  change. If a change would alter observable behavior, it is out of scope.
- **`make verify` green at every commit.** Run
  `cd /Users/gdc/deadreckon && make verify` — that is fmt-check, clippy `-D warnings`,
  the public-surface baseline check, `cargo test --workspace`, and the release build.
  Every single commit lands green. Unlike feature riders, this is the gate for *every*
  phase commit, not an end-of-run check.
- **Public-surface baseline is sacrosanct.** `/Users/gdc/deadreckon/tests/.public-surface-baseline`
  is unchanged across the whole goal **except** the single line P9 adds when
  `is_retryable_io_kind` is promoted to `pub` in core — and that re-baseline carries a
  written justification in the commit body. The binary crate is not in the baseline,
  so binary decomposition (P3–P8) cannot touch it; any *library* change that would
  alter a recorded path is rejected or explicitly re-baselined with written
  justification.
- **Moves are mechanical and behavior-free.** Code is relocated and made private
  (`mod foo;`) with intra-crate visibility widened only to `pub(crate)`, **never** to
  `pub`. No logic is edited in the same commit as a move. A move commit's diff is pure
  relocation plus visibility widening; a logic commit touches no module boundaries.
- **Lints stay satisfied.** No new `unsafe_code`, `unwrap_used`, `expect_used`,
  `unused_must_use`, `await_holding_lock`. The one new `expect` (P10 docs.rs regex)
  carries a `BUG:` message and is justified as a programmer-error invariant.
- **No `git push`.** Phased local commits only, conventional-commit messages, one
  CHANGELOG entry per phase.
- **No new public surface, no new library crate.** The binary stays a binary; no
  `pub run(cli) -> ExitCode` facade, no `deadreckon-cli` library crate. There is no
  second frontend or external consumer to justify a maintenance contract.
- **No V1 invention.** Anything beyond P1–P11 (handler traits, field encapsulation,
  `cli.rs` enum splitting, core `pub mod` tightening) goes to
  `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Vocabulary (use exactly)

- **characterization net** — the stdout/stderr golden snapshots in
  `/Users/gdc/deadreckon/crates/deadreckon/tests/characterization.rs` that pin
  current observable output **before** any refactor, so equivalence is proven, not
  asserted.
- **the monolith** — `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs`.
- **the dispatcher** — `main_inner`, the thin `match` that maps a parsed
  `cli.rs` command to `commands::<fn>`; it grows no logic, only delegates.
- **command family** — a cohesive group of handlers + their private helpers (chain,
  orchestrate, plan, campaign, attach, run, init), each landing in one
  `commands/` module or subdir.
- **the render layer** — the pure state→frame functions
  (`render_chain_attach`, `render_attach`, `render_plan_attach_text` family,
  `AttachTuiState`) that become unit-testable once separated from the terminal event
  loop, which stays in the command module.
- **move commit** vs **logic commit** — a move commit relocates code and widens
  visibility only; a logic commit edits behavior-equivalent code without crossing a
  module boundary. The two are never mixed in one commit.

## Why this is safe (the baseline argument — read before P3)

The four library crates are the public contract; the binary is glue. `make
public-surface` runs `cargo test -p deadreckon --test public_surface` against
`/Users/gdc/deadreckon/tests/.public-surface-baseline`, which records only library
crate paths. Therefore:

- **Binary moves (P3–P8) cannot change the baseline.** Widening a binary symbol from
  private to `pub(crate)` is invisible to the baseline; widening to `pub` would only
  matter if `main.rs` re-exported it, which we never do. Verification: after each
  binary phase, `git diff /Users/gdc/deadreckon/tests/.public-surface-baseline` is
  empty.
- **The one deliberate library change (P9)** promotes
  `deadreckon_core::error::is_retryable_io_kind` from crate-private to `pub`, adding
  exactly one recorded path. That single line is re-baselined in the same commit with
  a justification: "dedupe verbatim copies in providers/sandbox; behavior identical."
  This is the only baseline delta in the entire goal, and P9 is sequenced as the last
  code change before P11 docs so the delta sits isolated at the tip of history and is
  trivially revertible.

## Target module layout (the destination — concrete)

The decomposition lands this private tree under
`/Users/gdc/deadreckon/crates/deadreckon/src/`:

```
main.rs                      # thin: parse cli, build runtime, call main_inner, map ExitCode
commands/
  mod.rs                     # pub(crate) re-exports of the family modules; main_inner lives here or in main.rs
  chain/
    mod.rs                   # chain dispatch + shared chain helpers (chain_* ~2.9k lines today)
    plan.rs                  # chain plan/preview handlers
    status.rs                # chain status table rendering callers
    run.rs                   # chain run/resume handlers
  orchestrate.rs             # orchestrate_command + full-plan handlers + orchestrate helpers
  plan.rs                    # plan_command (draft/quiet) + plan helpers
  campaign.rs                # campaign_command + fan-out/rollup callers (engine stays in core)
  attach.rs                  # attach_command dispatch + the terminal event loops
  run.rs                     # run_command + run lifecycle helpers
  init.rs                    # init_command + setup wiring
  acceptance.rs              # acceptance_* handlers (~900 lines today)
  merge.rs                   # merge_command + compose callers (P7 helper lands in core/runtime as appropriate)
tui/
  mod.rs                     # pub(crate) render-layer facade
  attach_state.rs            # AttachTuiState + pure selection/state types
  render.rs                  # render_attach / render_chain_attach / render_plan_attach_text family (pure)
  keys.rs                    # pure handle_key reducers returning actions, no terminal
```

`cli.rs` is **unchanged** — the `Command` enum, `CampaignArgs`/`OrchestrateFullPlanArgs`
structs, and inline help consts stay exactly where they are. The dispatcher in
`main_inner` is a thin `match` over the existing `Command` enum that calls
`commands::<family>::<fn>(...)`. Nothing is re-exported from a binary `lib.rs` (the
binary's `lib.rs` does not gain new `pub` items).

## The inline test modules (the P3 lift — concrete)

These seven inline `#[cfg(test)]` modules in `main.rs` move out, **one module per
commit**, each to `/Users/gdc/deadreckon/crates/deadreckon/tests/<name>.rs` (or a
thin `#[cfg(test)]` sibling `src` module where a test reaches private items that
should stay private):

| Inline module (current main.rs line) | Target file | Notes |
|---|---|---|
| `acceptance_integrity_tests` (~10939) | `tests/acceptance_integrity.rs` | small; move first as the pattern-setter |
| `acceptance_render_tests` (~10995, ~6.9k lines) | `tests/acceptance_render.rs` | extract shared `acceptance_draft` factory to a fixtures module |
| `campaign_spawn_tests` (~17875) | `tests/campaign_spawn.rs` | overlaps with existing `tests/campaign.rs`; keep names distinct |
| `effortless_consistency_tests` (~18232, ~15k lines) | `tests/effortless_consistency.rs` | the largest; biggest line win |
| `flight_cli_tests` (~35511) | `tests/flight_cli.rs` | |
| `self_improve_pr_tests` (~35690) | `tests/self_improve_pr.rs` | |
| `tui_tests` (~35848, ~4.7k lines / 169 tests) | `tests/tui.rs` | imports 70+ internal symbols — moving this surfaces the exact `pub(crate)` set the carve needs |

`tui_tests` is moved **last** within P3 deliberately: it is the visibility canary. The
70+ symbols it references are exactly the set that command extraction (P4–P5) and TUI
extraction (P6) must make `pub(crate)`, so doing this move surfaces that set as a
mechanical compile-error checklist rather than guesswork.

## Characterization net (the safety harness — P1)

`/Users/gdc/deadreckon/crates/deadreckon/tests/characterization.rs` snapshots stdout
and stderr for representative invocations, using the `smoke` provider and `--sandbox
none` in a throwaway `DEADRECKON_HOME` tempdir. It is **full-binary capture** (run the
built binary, capture its streams) — not a pure-render-fn seam, because the render
functions are still inlined at P1 and the seam does not exist until P6. The attach
frame is captured via full-binary off-TTY/PTY output at P1; the pure render-fn unit
snapshot is added in P6 once the seam exists.

Representative invocations to pin (each its own golden):

- `plan --draft` and `plan --quiet` (plan-handler stdout shape).
- `orchestrate ... --preview --json` (preview JSON envelope).
- `chain ... status` (the chain status table).
- An `attach` off-TTY/plain summary frame (the largest render surface — see narrative
  note below).
- Two error messages that carry the `try:` footer (e.g. a campaign depth refusal and
  a not-a-git-repo refusal), pinning the canonical-pair footer text.

Golden files live under `/Users/gdc/deadreckon/crates/deadreckon/tests/goldens/`
(the dir already exists). A no-op re-run shows zero diffs. **No production code is
touched in P1.**

### Narrative coverage (fold-in from the critique)

The largest render surface is the narrative/attach projection. Two facts pin it:

1. The narrative render code currently embedded in `main.rs` lands in
   `/Users/gdc/deadreckon/crates/deadreckon/src/tui/render.rs` (P6), **not** in
   `/Users/gdc/deadreckon/crates/deadreckon/src/narrative.rs`. `narrative.rs` keeps
   its existing projection logic and stays `mod` (not `pub mod`); only the
   `main.rs`-embedded render functions relocate into `tui/`.
2. Because P6's render-fn seam does not exist at P1, the attach/narrative baseline in
   P1 is captured via **full-binary off-TTY output**, ensuring the narrative
   projection paths are snapshotted before they move. The pure render-fn unit snapshot
   is then added in P6 against that same fixed output.

## Test-helper consolidation (P2)

17 test files duplicate the same helpers (`repo_tempdir`, `deadreckon`,
`assert_success`, `stdout`, `stderr`). P2 adds
`/Users/gdc/deadreckon/crates/deadreckon/tests/common/mod.rs` with the single
canonical copy and switches each of the 17 files to `mod common; use common::*;`.
This is a **helper-source move only** — no assertion text changes, no test renamed, no
test split. After P2 every migrated file compiles and runs unchanged.

## Duplicated logic to unify (P7–P8 — concrete)

These are genuine triplications/duplications that force multi-site edits; they are the
only *logic* changes in the goal, each guarded by a characterization test written
first.

- **Merge loop (P7).** `compose_plan_merge_working`, `compose_dependency_source_dir`,
  and `compose_roots` reimplement the same iterate/hash/seen/conflict loop (~200
  lines, 3 places). Extract one parameterized helper taking a conflict-decision
  closure; rewrite the three callers to supply only their strategy. **Merge outcomes,
  conflict semantics, and error types are unchanged** — `PlanMergeStrategy` variants
  are untouched. A focused merge-conflict characterization test is written before the
  extraction.
- **Command existence (P8).** `start_command_exists` and `command_exists` are two
  PATH-check implementations. Unify into one helper that preserves the explicit-path
  branch `start` relies on; delete the redundant impl and repoint its one call site.

## Library cleanups (P9–P10 — concrete)

- **Dedupe `is_retryable_io_kind` (P9).** It is verbatim in 3 crates. Promote it to
  `pub` in `/Users/gdc/deadreckon/crates/deadreckon-core/src/error.rs`, re-export and
  reuse from `deadreckon-providers` and `deadreckon-sandbox` (signature and behavior
  identical), and delete the two copies. This is the **only** baseline change in the
  goal: re-baseline the single new core path with a written justification in the
  commit. Sequenced last among code phases so the delta is isolated.
- **Prune unused deps (P10).** Remove `tracing` from the three lib crates where it is
  declared-but-unused (`deadreckon-core`, `deadreckon-providers`, `deadreckon-sandbox`)
  and `chrono` from `deadreckon-providers`. Keep both at the workspace root for crates
  that use them. `cargo build` confirms each removal.
- **Remove confirmed dead code (P10).** Delete `_is_git_binary` and the `ui.rs`
  helpers confirmed unused (`kv_block`, dead `write` helper). **Keep** the
  intentionally-reserved `campaign`/`fork` `#[allow(dead_code)]` items, but add a
  clearer comment naming why each is reserved.
- **Harden docs.rs regex init (P10).** Replace the panic-on-bad-pattern with `expect`
  carrying a `BUG:` message, and add a unit test that compiles both patterns so a
  malformed pattern fails the test, not a user run.
- **Targeted allocation nits (P10).** Mechanical, semantics-preserving:
  `plan_event_bus.rs` store `&str` instead of `String` at the line storing a cloned
  key, and hoist the two cloned `String` keys out of the map closure; whitespace
  compaction (`split_whitespace().collect::<Vec>().join(" ")` → single-pass fold) and
  redundant `.as_str().to_string()` removal in `narrative.rs` and the two former
  `main.rs` sites. **These sites are located by SYMBOL at execution time, not by the
  stale line numbers** (`main.rs:23210`/`35485`/`plan_event_bus.rs:368`/`:249-262`)
  — P3–P6 will have relocated the `main.rs` offsets by P10. Only touch a site after
  reading its surrounding scope and confirming the original value is not needed after
  the call. Output strings stay byte-identical (covered by the characterization net).

## Rejected as needless churn (do NOT do these)

The next agent must **not** attempt the following. Each was considered and rejected
with a baseline-grounded or pressure-grounded reason; log none of them as work, only
to `V1-CANDIDATES.md` if a real need later appears.

1. **Make `deadreckon-core`'s `pub mod` declarations private and route everything
   through flat re-exports.** Consumers already deep-path into core
   (`deadreckon_core::campaign::ENV_SUB_RESULT`, `::plan::PLAN_NARRATIVE`,
   `::plan::PLAN_DOCS_DIR`, `::campaign::read_sub_result`, `::campaign::SubResult`,
   `::docs::*` in runtime), and several of those items are NOT in the flattened
   re-export list. This breaks internal callers AND rewrites ~268 baseline lines for
   zero product benefit. **Reject.**
2. **Encapsulate `Chain`/`Plan`/`PlanTask`/`ChainStep` fields behind semantic
   transition methods** (`mark_step_running`, etc.). ~74+ call sites in the monolith, a
   public-surface change on the most-used core types, high break risk for
   forward-flexibility the product does not need. The audit rates it risk:high/large.
   **Reject as churn.**
3. **Add `#[non_exhaustive]` / accessors to `SpendEstimate`, `ProviderRouteInfo`, or
   wrap `PlanTask.depends_on` in a newtype.** Speculative API-evolution insurance with
   no concrete pressure and a baseline impact. **Reject until a real evolution need
   appears.**
4. **Split `cli.rs`'s 45-variant `Command` enum and inline help consts into per-family
   files with re-exports.** A rename/relocation diff that keeps the enum identical to
   callers; the dispatcher refactor (P4–P5) delivers the navigability win without
   churning the parse layer. **Reject.**
5. **Introduce a uniform `CommandHandler` trait / `cli()->exec()` shape across all
   handlers.** Cosmetic standardization with one caller each; the thin dispatch match
   already unblocks middleware. **Reject.**
6. **Move binary CLI logic into a fat `lib.rs` with `pub run(cli) -> ExitCode` for
   external reuse.** No second frontend or external consumer exists; this adds a public
   surface and maintenance contract for hypothetical reuse. **Defer/reject until a real
   library consumer exists.**
7. **Add `#[source]` to `ProviderError::Http`/`Cli` and adopt sysexits granular exit
   codes.** Behavior-adjacent (changes error chains / exit signals) with no current
   consumer inspecting them; out of scope for an equivalence-preserving refactor.
   **Reject.**
8. **Reorganize large integration test files (`orchestrate.rs`, 176 tests) into feature
   submodules and add per-test doc comments.** Pure cosmetic reshuffling neither clippy
   nor rustfmt requested, inflating diffs reviewers must re-verify. **Reject.**

## Phases (eleven)

Each phase: write the named depth/characterization test(s) **first** and watch them
fail (RED); refactor; bring `cd /Users/gdc/deadreckon && make verify` green (fmt-check
+ clippy `-D warnings` + public-surface + `cargo test --workspace` + release build);
conventional-commit local commit; one CHANGELOG entry per phase. For multi-move phases
(P3, P5) `make verify` is green after **each** move commit, not just at phase end.
Smokes use the `smoke` provider with `--sandbox none` in a throwaway
`DEADRECKON_HOME`.

### P1 — Characterization net (RED) — `tests/characterization.rs`

Add the golden stdout/stderr snapshots for the representative invocations (plan
`--draft`/`--quiet`, orchestrate `--preview --json`, chain status, attach off-TTY
frame, two `try:`-footer errors). Capture is full-binary; goldens land under
`/Users/gdc/deadreckon/crates/deadreckon/tests/goldens/`. No production code touched.

Characterization tests (`tests/characterization.rs`):
- `plan_draft_stdout_matches_golden`
- `plan_quiet_stdout_matches_golden`
- `orchestrate_preview_json_matches_golden`
- `chain_status_table_matches_golden`
- `attach_off_tty_frame_matches_golden`
- `error_footers_match_canonical_goldens`

Verify: `make verify` green; goldens committed as the baseline; a no-op re-run shows
zero diffs.

### P2 — Test-helper consolidation — `tests/common/mod.rs`

Add `tests/common/mod.rs` with the canonical `repo_tempdir`/`deadreckon`/
`assert_success`/`stdout`/`stderr`; migrate the 17 duplicated test files to import it.
Helper-source move only — no assertion changes, no renames, no splits.

Depth tests:
- `common_helpers_compile_and_are_reused` (a thin test asserting the shared module is
  the one path; mostly proven by the 17 files compiling against it)

Verify: `make verify` green; `cargo test --workspace` runs the same test count with
the same names; `git diff` shows pure helper relocation.

### P3 — Lift inline test modules out of the monolith (one per commit)

Move the seven inline `#[cfg(test)]` modules to `tests/` per the table above, widening
the minimum set of referenced items to `pub(crate)`. Extract shared fixtures
(`acceptance_draft`, `chain_fixture`) to a `#[cfg(test)]` fixtures module so they are
not duplicated. `tui_tests` moves **last** as the visibility canary. One module per
commit; `make verify` green after each.

Depth tests:
- (the moved tests are themselves the depth tests) — `cargo test --workspace` runs the
  same test count with the same names after each move
- `monolith_line_count_drops_below_threshold` (an optional guard test asserting
  `main.rs` is now under ~12k lines, pinning the shrink)

Verify: `make verify` green per move; `git diff` per commit shows pure relocation plus
`pub(crate)` widening; test count and names unchanged.

### P4 — `commands/` skeleton + thin `main_inner` dispatch + chain family first

Create `src/commands/mod.rs` and the thin `main_inner` dispatcher; move the **chain**
family (`commands/chain/`) first as the pattern-setter, widening cross-module refs to
`pub(crate)`. `cli.rs` unchanged.

Depth tests:
- `chain_status_table_matches_golden` (P1 golden re-run, unchanged)
- `dispatcher_routes_chain_to_commands_chain` (a thin test that the chain branch of
  `main_inner` reaches `commands::chain`)

Verify: P1 characterization goldens unchanged; `make verify` green; smoke `chain plan`
in a throwaway repo renders identically; `git diff /Users/gdc/deadreckon/tests/.public-surface-baseline`
empty.

### P5 — Move remaining command families (one cohesive commit each)

Move orchestrate, plan, campaign, run, init, attach, acceptance, merge into their
`commands/` modules per the layout, one family per commit, widening refs to
`pub(crate)`. The terminal event loops stay in `commands/attach.rs` (the pure render
fns move in P6).

Depth tests (per family, the P1 goldens re-run unchanged):
- `plan_draft_stdout_matches_golden`, `plan_quiet_stdout_matches_golden`
- `orchestrate_preview_json_matches_golden`
- `attach_off_tty_frame_matches_golden`
- `error_footers_match_canonical_goldens`

Verify: characterization goldens unchanged after each family; `make verify` green per
family; smoke `init`/`run`/`plan` in a throwaway repo behave identically; baseline
diff empty.

### P6 — Extract the pure TUI render layer into `src/tui/`

Move render/state types and pure `state→frame` functions
(`AttachTuiState`, `render_attach`, `render_chain_attach`, the
`render_plan_attach_text` family, the narrative-projection render code embedded in the
monolith) into `src/tui/` (`render.rs`, `attach_state.rs`, `keys.rs`),
`pub(crate)` where shared. The terminal event loop stays in `commands/attach.rs`,
calling those render fns. Add a couple of unit tests exercising a render fn against a
fixed state (now enabled by the separation), including the narrative-projection
render-fn unit snapshot deferred from P1.

Depth tests:
- `attach_off_tty_frame_matches_golden` (P1 golden, unchanged)
- `render_attach_frame_unit_snapshot` (new pure render-fn unit snapshot)
- `render_chain_attach_unit_snapshot`

Verify: characterization frame snapshot unchanged; `make verify` green; manual attach
smoke renders identically; baseline diff empty.

### P7 — Characterization-guarded merge-loop dedupe

Write a merge-conflict-path characterization test first, then extract the parameterized
merge helper unifying `compose_plan_merge_working`, `compose_dependency_source_dir`,
and `compose_roots`. Outcomes, conflict semantics, and error types unchanged;
`PlanMergeStrategy` untouched.

Depth tests:
- `merge_conflict_path_characterization` (RED first, pinning current conflict output)
- `compose_helper_extracted_without_changing_merge_outcomes`

Verify: existing orchestrate/merge integration tests pass unchanged; `make verify`
green; baseline diff empty.

### P8 — Unify command-existence checks

Unify `start_command_exists` and `command_exists` into one helper preserving the
explicit-path branch `start` uses; delete the redundant impl; repoint its call site.

Depth tests:
- `command_exists_resolves_path_and_bare_name`
- `start_command_exists_explicit_path_branch_preserved`

Verify: `make verify` green; baseline diff empty.

### P9 — Dedupe `is_retryable_io_kind` (the one re-baseline)

Promote `is_retryable_io_kind` to `pub` in
`/Users/gdc/deadreckon/crates/deadreckon-core/src/error.rs`; re-export and reuse from
providers/sandbox; delete the two copies. Re-baseline the single new core path in
`/Users/gdc/deadreckon/tests/.public-surface-baseline` **in this commit** with a
written justification in the commit body. This is the only baseline delta in the goal.

Depth tests:
- `is_retryable_io_kind_behaves_identically_across_crates` (parameterized over the IO
  error kinds the three former copies handled)

Verify: `make verify` green; the public-surface check passes against the re-baselined
file; the commit body justifies the single new path; this is the last code change
before docs.

### P10 — Dep prune, dead-code removal, regex hardening, allocation nits

Remove unused `tracing` (3 lib crates) and `chrono` (providers); delete confirmed dead
code (`_is_git_binary`, unused `ui.rs` helpers); keep reserved `#[allow(dead_code)]`
items with clearer comments; harden the docs.rs regex with `expect`+`BUG:` and a
compile-the-patterns unit test; apply the targeted allocation nits **located by
symbol, not by stale line number**.

Depth tests:
- `docs_regex_patterns_compile` (unit test compiling both patterns)
- (dep/dead-code removals proven by `make verify` clippy + build, no new test needed
  beyond confirming green)

Verify: `make verify` green; `cargo build` confirms dep removals; clippy confirms
dead-code removals; characterization goldens unchanged (allocation nits are output-
neutral); baseline diff empty (these are non-`pub` or dep-only changes).

### P11 — AS-BUILT §38 + CHANGELOG + V1-CANDIDATES (doc only; no depth test)

- Insert new top-level section into
  `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` (next number after §37):

  ```
  ## 38. Binary Module Layout (post-decompose)

  38.1 Why main.rs was split (40.6k lines; binary not in the baseline)
  38.2 The characterization net (tests/characterization.rs) and goldens
  38.3 commands/ tree: one module per family behind main_inner dispatch
  38.4 tui/ render layer: pure state→frame fns vs the event loop
  38.5 The pub(crate) visibility discipline (never pub on the binary)
  38.6 The single re-baselined library path (is_retryable_io_kind)
  38.7 Duplicated logic unified (compose_* merge helper, command-exists)
  38.8 What was deliberately NOT done (rejected-as-churn pointers)
  ```

  Update the "What's Built vs Scaffolding-Thin" list: note that the binary is now
  modular; state plainly that `cli.rs`, the `Command` enum, all verbs, all output, and
  the public surface are unchanged.
- Append to `/Users/gdc/deadreckon/docs/CHANGELOG.md`:

  ```
  ## Decompose (maintainability refactor) — 2026-05-29

  - Split the 40.6k-line crates/deadreckon/src/main.rs into private commands/ and
    tui/ modules behind a thin main_inner dispatcher; behavior byte-identical,
    proven by a new CLI-output characterization net (tests/characterization.rs).
  - Lifted seven inline #[cfg(test)] modules out of main.rs into tests/.
  - Unified the triplicated merge-compose loop and the two command-exists impls.
  - Deduped is_retryable_io_kind into deadreckon-core (single public-surface
    re-baseline, justified); pruned unused tracing/chrono deps; removed dead code;
    hardened the docs.rs regex init.
  - No verb, flag, file, field, or output-string change.
  ```

- Log to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` (only items deferred during
  execution, plus the rejected-as-churn list as explicit "not now" pointers): core
  `pub mod` tightening with flat re-exports; Chain/Plan field encapsulation behind
  transition methods; a uniform `CommandHandler` trait; a `pub run(cli) -> ExitCode`
  library facade / `deadreckon-cli` crate; `#[source]` on `ProviderError` + sysexits
  exit codes; `cli.rs` enum-per-family split; integration-test submodule reorg.

## Out of scope (explicitly rejected or V1 candidates)

- Any verb, flag, file, field, or output-string change (this is a refactor).
- Core `pub mod`→`mod` visibility tightening (rejected; breaks deep-path callers and
  rewrites the baseline).
- Chain/Plan field encapsulation behind transition methods (rejected; ~74 call sites,
  baseline change, high risk).
- A `CommandHandler` trait / uniform `cli()->exec()` shape (rejected; cosmetic).
- A `lib.rs` `pub run(cli)` facade or a `deadreckon-cli` library crate (rejected; no
  consumer).
- `#[source]` on `ProviderError` and sysexits exit codes (rejected; behavior-adjacent).
- Splitting `cli.rs`'s `Command` enum into per-family files (rejected; rename churn).
- Reorganizing `orchestrate.rs` (176 tests) into submodules / adding per-test doc
  comments (rejected; cosmetic).

## Integration matrix

| Aspect | Before | After (this rider) |
|---|---|---|
| `main.rs` size | 40,597 lines | thin (~hundreds); families under `commands/`, render under `tui/` |
| inline test modules | 7 in `main.rs` (~29k lines) | moved to `tests/` |
| dispatch | inline `match` + free fns | `main_inner` → `commands::<family>::<fn>` |
| render fns | inlined in handlers | pure fns in `tui/render.rs`, unit-tested |
| stdout/stderr/exit/side-effects | (live) | byte-identical, pinned by `tests/characterization.rs` |
| `cli.rs` / `Command` enum / verbs | (live) | unchanged |
| public-surface baseline | recorded | unchanged except one justified P9 line |
| merge-compose loop | 3 copies | 1 parameterized helper |
| `is_retryable_io_kind` | 3 verbatim copies | 1 in core, re-exported |

## Engineering invariants (do not violate)

- **Behavior is byte-identical.** stdout, stderr, exit codes, and file/state side
  effects do not change. Guarded by `tests/characterization.rs` captured in P1 before
  any move.
- **`make verify` is green at every commit** — including each individual move commit in
  P3 and P5, not just at phase end.
- **The public-surface baseline changes exactly once,** in P9, with a written
  justification in the commit body. Every other phase leaves
  `/Users/gdc/deadreckon/tests/.public-surface-baseline` byte-identical; assert this
  with an empty `git diff` on that file per commit.
- **Move commits and logic commits never mix.** A move commit relocates code and widens
  visibility to `pub(crate)` only; it edits no logic. A logic commit (P7–P10) crosses
  no module boundary.
- **Visibility is widened to `pub(crate)`, never `pub`,** on binary symbols. The binary
  `lib.rs` gains no `pub` items.
- **One depth/characterization test before each phase implementation.** A phase whose
  tests all started green never failed; that is a smell.
- **No new crates.** Tier 1 only (`insta` or plain golden files for characterization —
  prefer golden files if `insta` is not already a dev-dep, to avoid a new dependency).
  Tier 2/3: same blocks as prior riders.
- **No silent expansion.** Anything beyond P1–P11, and every rejected-as-churn item,
  goes to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`, never into the diff.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth/characterization tests passing, `make verify` green,
  and a CHANGELOG entry naming the SHA.
- If a move reveals that a symbol genuinely cannot stay `pub(crate)` without a `pub`
  re-export (i.e. an external test or the public surface needs it), **stop** and log
  the decision in `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` rather than silently
  widening the surface; the baseline is changed only by the one justified P9 line.
- If P7's merge-helper extraction cannot preserve byte-identical conflict output, stop
  and reduce scope to the two trivially-identical callers, logging the third in
  `V1-CANDIDATES.md`.
- Optional after P11: an asciinema cast under `/Users/gdc/deadreckon/` showing
  `make verify` green and a smoke `run`/`chain`/`attach` rendering identically
  pre/post decompose. Skip if not worth it.
