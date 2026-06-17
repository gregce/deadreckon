# deadreckon — Orchestrated Narration Rider (full narration for orchestrate & campaign)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-06-17-1525-deadreckon-orchestrate-campaign-narration-goal.md`.
It supersedes nothing in prior riders — their invariants still apply, notably:

- `2026-06-15-1433-deadreckon-live-narrator-rider.md` — the Live Narrator engine, cadence, continuity, deterministic floor, spend isolation, and surfaces. This rider EXTENDS that engine to orchestrate/campaign children; it does not change the engine.

This rider adds: per-child file-backed narration for orchestrated/campaign subprocesses, the two `extend` wirings, CLI flag propagation down the fork/sub-orchestrator argv, a `spend_summary` kind-isolation fix, plan-level surfacing reliability, and Option D (parent aggregate stderr line + a campaign Narrative view).

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime `<run_root>` under `$DEADRECKON_HOME` (default `~/.deadreckon`).

## Posture (decided — do not redesign)

- **Maturity stays stable track** (0.2.0 shipped; this lands as `0.3.0 — Orchestrated Narration`). Additive only.
- **No `PipelineState`/`RunLoopConfig` schema breakage.** `RunLoopConfig.narrate` already exists. Per-child narration state stays file-backed under `<run_root>/narrative/`.
- **Children narrate to files, never to their pipes.** A child's stdout is scraped for the run id (`parse_started_run_id`) and its stderr is captured for failure summaries; a beat on either channel corrupts the parent. Child narration is FILE-ONLY (`snapshots.jsonl`).
- **The engine is reused unchanged.** `NarratorEngine`, `build_narration`, `NarratorCtx`, `NarratorHandle`, `NarratorLedger`, `resolve_narrator_backend`, the `snapshots.jsonl` append path, the plan `agent_table` per-child fold, and post-hoc `live_narrative_digest` seeding are all reused as-is. Do not fork the engine.
- **Deterministic floor stays the no-provider floor.** Default child backend is the floor ($0, no auth probe) unless `--narrator-model` is explicitly passed.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Anything beyond P1–P11 → `docs/V1-CANDIDATES.md`.
- **Edits stay inside `/Users/gdc/deadreckon`.**

## Why children get zero beats today (the spec to fix)

Children are SUBPROCESSES, not in-process loops. `fork_command` (plan.rs:1593) spawns each ready task via `run_plan_child` (plan.rs:2557) as `std::process::Command::new(current_exe())`:
- Coder / full-plan child → `deadreckon run <prompt> --from <src> --yes --no-confirm --no-hints --no-docs --provider <p> [--model m]` (plan.rs:2596-2611). `run.rs` HAS the narrator (399-465) but `resolve_narrator_config(io::stdin().is_terminal(), narrate=false, …)` returns `None` (narrator.rs:50-57) because the child's stdin is piped and no `--narrate` is passed.
- Reviewer child (single dep) → `deadreckon extend <parent_run_id> <prompt> --no-docs` (plan.rs:2589-2594), re-entering `extend_command` (lifecycle.rs:1573) or `extend_worktree_command` (lifecycle.rs:1831), both building `RunLoopConfig` with `event_sender: None` (1586/1844) and `narrate: None` (1588/1846).

Campaign nests this: `campaign` spawns `orchestrate full-plan` via `build_sub_orchestrator_command` (campaign.rs:37-91) → fork → `run`/`extend` leaves. No narrate flag exists on orchestrate/campaign to thread down.

## Data model (files, not fields)

No new persistent state. Children write the EXISTING schema-2 `NarrativeSnapshot` (with the `live` beat) to `<run_root>/narrative/snapshots.jsonl` via the existing engine. The parent reads those files; it stores nothing new.

`NarratorConfig` (deadreckon-runtime/turn_loop.rs) is reused; a child sets `foreground=false, headless_append=false` so `render_beat` (narrator.rs) no-ops on both surfaces while `commit_model_beat`/`commit_floor_beat` still append to `snapshots.jsonl`.

## Child-narration resolution (pseudocode — match this)

```
// NEW in narrator.rs — does NOT replace resolve_narrator_config (that keeps its dr-run TTY contract).
fn resolve_narrator_config_for_child(narrate, no_narrate, model_override) -> Option<NarratorConfig>:
    if no_narrate || !narrate: return None        // child silent unless parent opted in
    return Some(NarratorConfig {
        foreground: false,        // headless child: no calm block
        headless_append: false,   // FILE-ONLY: never touch child stdout/stderr
        model_override,
        ..NarratorConfig::default()
    })
```

Backend for a child: floor unless `model_override` is `Some` (then the metered model). The parent resolves the backend once and passes `--narrator-model <id>` + sets `DEADRECKON_AUTH_PROBE=0` on the child env so children do not each shell `claude auth status`. A child whose provider is `smoke` forces the floor (extend has no `--smoke` flag; detect `provider == "smoke"`).

## Flag signatures

```
deadreckon extend <id> <goal>        [--narrate] [--no-narrate] [--narrator-model <id>]
deadreckon orchestrate <sub> …       [--narrate] [--no-narrate] [--narrator-model <id>]
deadreckon campaign <goal> …         [--narrate] [--no-narrate] [--narrator-model <id>]
```

Validated by `crate::narrator::validate_narration_flags` (run.rs:39 pattern) and `--narrator-model` by `narrator_model_known`/`narrator_model_refusal`. `--narrate` on a parent enables FILE-ONLY child narration by default and the parent aggregate line (P9). Without it, children stay silent (no behavior change, no cost).

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them fail; implement; green on `cargo fmt --check` + `cargo clippy --workspace -- -D warnings` + focused `cargo test`; conventional-commit; one-line CHANGELOG entry. The project clippy gate is `cargo clippy --workspace` (NOT `--all-targets`); `clippy.toml` allows expect/unwrap in tests.

### P1 — Child resolve + shared `build_run_narration` helper (foundation, no behavior change to `dr run`)

- Add `resolve_narrator_config_for_child` to `crates/deadreckon/src/narrator.rs` per the pseudocode. Leave `resolve_narrator_config` untouched.
- Extract the narrator-build block at `crates/deadreckon/src/commands/run.rs:405-428` into `build_run_narration(home, config_path, run_id, run_root, smoke_or_floor, config: Option<NarratorConfig>) -> (Option<broadcast::Sender<RunEvent>>, Option<NarratorHandle>)` in `narrator.rs` (smoke/`provider==smoke` ⇒ `DeterministicFloor`, else `resolve_narrator_backend`; build `NarratorCtx`; `build_narration`). Refactor `run.rs` to call it (behavior-preserving).

Depth tests (`crates/deadreckon/src/narrator.rs` tests):
- `resolve_narrator_config_for_child_returns_some_file_only_off_tty`
- `resolve_narrator_config_unchanged_for_dr_run_tty_matrix`
- `child_narrator_defaults_to_deterministic_floor_when_no_narrator_model`

### P2 — Wire `extend_command` (in-place) + thread `ExtendCommandArgs` flags

- Add `narrate`/`no_narrate`/`narrator_model` to `ExtendCommandArgs` (cli.rs ~2403) and the `Extend` clap variant; validate via `validate_narration_flags`.
- Before `lifecycle.rs:1573`, resolve a config: interactive `dr extend` uses `resolve_narrator_config(io::stdin().is_terminal(), …)`; a reviewer child (off-TTY, `--narrate` passed) lands in `resolve_narrator_config_for_child`. Call `build_run_narration` from `state.run_id`/`state.run_root`/`paths.config_path()`/`paths.home()` (`provider==smoke` ⇒ floor). Replace `event_sender: None` (1586) and `narrate: None` (1588). Insert `handle.shutdown().await` after the awaited loop and BEFORE `state.child_pids.clear()`.

Depth tests:
- `extend_command_in_place_reviewer_child_narrates_when_narrate_passed`
- `extend_narrator_handle_shutdown_runs_before_lock_release`

### P3 — Wire `extend_worktree_command` + callers

- Mirror P2 at the worktree site (lifecycle.rs:1831): resolve config, `build_run_narration`, replace `event_sender: None` (1844) / `narrate: None` (1846), `shutdown().await` before `child_pids.clear()`. Forward the narrate fields into `ExtendWorktreeArgs` (lifecycle.rs:1678).
- Update extend callers to pass the flags: `main.rs` (~1183, ~11544) and `start.rs` (~2682, default false/false/None).

Depth tests:
- `extend_worktree_command_reviewer_child_narrates_when_narrate_passed`
- `headless_child_narration_keeps_stdout_clean_so_parent_scrapes_run_id`
- `headless_child_narration_keeps_stderr_clean_no_failure_summary_pollution`

### P4 — Orchestrate + campaign CLI flag parity

- Add `narrate`/`no_narrate`/`narrator_model` to the `Orchestrate` and `Campaign` clap variants and their Args structs (cli.rs, mirroring the run pattern ~829-836); validate via `validate_narration_flags`; refuse a bad `--narrator-model` with the `deadreckon models` hint.
- Thread the intent into `fork_command` (orchestrate) and the campaign fork path. The parent resolves the backend ONCE (so children inherit `--narrator-model` and skip auth probes).

Depth tests:
- `orchestrate_rejects_conflicting_narrate_flags`
- `campaign_narrator_model_validated_against_catalog`

### P5 — `run_plan_child` appends `--narrate` to child argv

- At `plan.rs:2596-2636`, when narration is enabled append `.arg("--narrate")` to BOTH the `extend` branch (2589-2594) and the `run` branch (2596-2611), and `.arg("--narrator-model").arg(id)` when set. Set `.env("DEADRECKON_AUTH_PROBE", "0")` at plan.rs:2585-2587 when narrating. Children resolve via `resolve_narrator_config_for_child` (file-only).
- Update the pinned child-argv tests (`plan.rs` ~3009 `model_argv_tests`) in lockstep.

Depth tests:
- `orchestrate_full_plan_with_narrate_appends_narrate_flag_to_each_child_argv`
- `headless_child_run_subprocess_writes_live_beat_to_its_snapshots_jsonl`
- `plan_child_argv_pinned_test_updated_for_narrate_flag`

### P6 — Campaign depth: `build_sub_orchestrator_command` propagation

- At `campaign.rs:37-91`, append `--narrate`/`--narrator-model` to the `orchestrate full-plan` sub argv so campaign → orchestrate → run/extend leaves narrate end-to-end. Update `campaign.rs` ~2246 `model_argv_tests` in lockstep.

Depth tests:
- `campaign_narrate_propagates_through_sub_orchestrator_to_leaf_run_argv`

### P7 — `spend_summary` kind isolation (latent fix)

- In `crates/deadreckon-core/src/state.rs` `spend_summary` (300-340), skip non-loop rows in the accumulation loop (~316): `if record.kind != "loop" { continue; }`, and take `total_usd` from the last `kind=="loop"` row (the existing fallback to `state.total_spend_usd` at ~336 covers the empty case). Narrator rows (`kind:"narrator"`, narrator.rs) must not inflate tokens/turns/wall or overwrite `total_usd`.
- Add a `tests/spend_summary.rs` case feeding a `kind:"narrator"` row.

Depth tests:
- `spend_summary_excludes_kind_narrator_rows_from_total`
- `spend_summary_total_usd_taken_from_last_loop_row`

### P8 — Plan-level surfacing reliability

- Cap the plan `agent_table` render (narrative.rs:1103-1115) to `narrate_lines` active children with a `+N more` overflow line (rider Q5).
- Fix the Live/Deterministic mask: `read_latest_snapshot` (narrative.rs:1157) returns only the last row, so an attach-time `Deterministic` projection appended after a `Live` beat hides it. Either don't append a `Deterministic` row when the latest existing row is a `Live` beat, OR have `latest_child_narrative_snapshot` (narrative.rs:2866) prefer the latest `source==Live` row.

Depth tests:
- `plan_projection_surfaces_child_live_headline_one_line_per_active_child`
- `plan_agent_table_caps_at_narrate_lines_with_plus_n_more_overflow`
- `attach_time_deterministic_projection_does_not_mask_prior_live_beat`

### P9 — Option D1: parent aggregate stderr line

- Under `--narrate`, the orchestrate/campaign PARENT tails each active child's `<run_root>/narrative/snapshots.jsonl` (reuse `plan_event_bus::JsonlTail`, already imported in campaign.rs:7) and prints a calm, bounded aggregate to **STDERR** — one line per active child (`child label · latest live headline`), capped at `narrate_lines` with `+N more`. Never stdout (carries plan progress + run-id scraping). Refresh as children advance; tolerate partial/locked reads so it never races the fork loop's own file reads or crashes the parent.

Depth tests:
- `parent_aggregate_stderr_prints_one_capped_line_per_active_child`
- `parent_aggregate_never_writes_to_stdout`

### P10 — Option D2: campaign Narrative view

- Add `build_campaign_projection` in `narrative.rs` (analogous to `build_plan_projection`) that aggregates each sub-goal/sub-orchestrator's latest child live beat into one rolling line per sub-goal (reuse `narrative_plain_lines`/`agent_table` shape).
- Wire a Narrative view into campaign attach (`commands/attach.rs` + the campaign TUI path, which today renders only sub-goal rollups) at parity with plan attach (the `n` toggle / `--view narrative` plain/json paths). Headless `--plain/--json` renders the latest projection with no provider call (preserve that invariant).

Depth tests:
- `campaign_narrative_projection_aggregates_child_live_beats`
- `campaign_attach_has_narrative_view_at_parity_with_plan`

### P11 — Architecture doc + V1-CANDIDATES + CHANGELOG (doc only; no depth test)

- Insert `## 45. Orchestrated Narration (every child narrates; parent aggregate; campaign view)` into `docs/AS-BUILT-ARCHITECTURE.md`:
  ```
  45.1 Subprocess model: children are deadreckon run/extend processes; narration is per-child file-backed
  45.2 Child resolve (file-only) + shared build_run_narration; the two extend wirings
  45.3 Flag propagation: extend/orchestrate/campaign --narrate -> run_plan_child / build_sub_orchestrator_command argv
  45.4 Backend: floor default, parent-resolved model, DEADRECKON_AUTH_PROBE=0 to avoid the probe storm
  45.5 Plan surfacing (agent_table cap + Live/Deterministic interleaving fix) and spend_summary kind isolation
  45.6 Option D: parent aggregate stderr line; campaign Narrative view
  ```
- Correct §44: the "run/orchestrate/campaign" claim was true only for run; update it to point at §45 for orchestrate/campaign coverage. Update §22 "What's Built vs Scaffolding-Thin": orchestrate/campaign narration now shipped.
- Append `## 0.3.0 — Orchestrated Narration` to `CHANGELOG.md`.

## Integration matrix

| Surface | Child narration | Parent aggregate (D1) | Attach Narrative view |
|---|---|---|---|
| `dr run` (TTY) | calm foreground block (§44) | n/a | run Narrative view |
| `dr extend` reviewer child | file-only `snapshots.jsonl` | via orchestrate parent | (plan view) |
| `dr orchestrate --narrate` | each child file-only | stderr, capped | plan Narrative (per-child agent_table) |
| `dr campaign --narrate` | each leaf file-only | stderr, capped | campaign Narrative (D2) |
| No `--narrate` | silent (unchanged) | none | deterministic projection only |
| No provider | deterministic floor | floor headlines | floor projection |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| `--narrate and --no-narrate are mutually exclusive` (orchestrate/campaign/extend) | `try: pass only one of --narrate / --no-narrate` |
| `unknown narrator model '<X>'` | `try: deadreckon models` |

## Config additions

Reuses the `[defaults] narrate_*` knobs from the Live Narrator rider (no new knobs required). `narrate_lines` bounds both the plan agent_table cap (P8) and the parent aggregate (P9).

## Out of scope (explicitly → V1-CANDIDATES)

- A shared, cross-process narrator ledger / IPC across children (children stay isolated; per-child budget only).
- A live in-process RunEvent stream in the parent (parent observes via file tail only).
- Reading the `[defaults] narrate_*` knobs from `config.toml` (still deferred from the Live Narrator rider).
- Narrator wiring for the merge/repair child spawn path (`MergeRepairRunDiscovered`) — note it as a follow-up if a phase touches it.
- Persistent streaming CLI session for the narrator backend.

## Dependencies (Tier 1 / 2 / 3 policy)

- **Tier 1 (utility, free):** none new — `tokio`, `serde`, `which`, and `plan_event_bus::JsonlTail` are present.
- **Tier 2 (architectural, log to `DEPENDENCIES.md`):** none expected.
- **Tier 3 (blocked):** same blocks as prior riders.

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes**; child narration state is files under `<run_root>/narrative/`.
- **Children never write beats to stdout or stderr** — file-only. This is depth-tested (`*_keeps_stdout_clean`, `*_keeps_stderr_clean`, `parent_aggregate_never_writes_to_stdout`); a regression here breaks the parent's run-id scrape and failure capture.
- **Shutdown ordering mirrors `run.rs:463`**: `handle.shutdown().await` after the awaited loop, before `state.child_pids.clear()`/`save_state`/`lock.release`. Forgetting it leaks the engine task or drops the final beat.
- **The narrator engine is not forked or modified** — only resolution, wiring, and surfacing change.
- **One depth test before each phase implementation.** A phase whose tests were never red is suspect.
- **Pinned child-argv tests (`plan.rs` ~3009, `campaign.rs` ~2246) are specs** — update them in the same commit that changes the argv.
- **No silent expansion.** Anything beyond P1–P11 → `V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push`.
- Conventional commits scoped: `feat(narrator):` / `feat(orchestrate):` / `feat(campaign):` / `fix(state):` per phase; `docs(goals):` for this pair.
- Each phase ends with its named depth tests passing and a CHANGELOG line.
- After P11, optionally capture an asciinema cast of an orchestrate `--narrate` run showing the parent aggregate + plan attach Narrative view.
- If a phase reveals a V1-architecture decision (e.g., the merge/repair path needs a different model, or campaign attach needs a new TUI mode), stop and log it in `V1-CANDIDATES.md`.

## Open questions (decide/confirm during implementation; log the decision)

1. **Default child backend** — floor unless `--narrator-model` is passed (lean: yes, $0 + no probe storm). Confirm.
2. **Parent-resolved vs per-child backend** — parent resolves once and threads `--narrator-model` + `DEADRECKON_AUTH_PROBE=0` (lean: yes). Confirm no auth-state divergence across children.
3. **`start.rs` non-interactive extend caller** — narration on by default (`!quiet`) or silent unless flagged? Lean: silent unless flagged.
4. **Parent aggregate refresh cadence** — tail-driven on each child beat vs a fixed interval; bound to `narrate_lines`. Lean: tail-driven, coalesced.
5. **Campaign Narrative aggregation granularity** — one rolling line per sub-goal vs per leaf child. Lean: per sub-goal (drill to plan/child for detail).
6. **Live/Deterministic mask fix placement** — suppress the Deterministic append vs prefer `source==Live` in `latest_child_narrative_snapshot`. Lean: prefer `source==Live` (least invasive, keeps the audit trail whole).
