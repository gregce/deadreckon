# deadreckon — Orchestrate Campaign Rider (one task spawns N orchestrators, depth-capped at 2)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-28-1841-deadreckon-campaign-goal.md`.
It supersedes nothing in prior riders (notably
`2026-05-11-1444-deadreckon-orchestrate-rider.md`,
`2026-05-16-1122-deadreckon-semantic-merge-repair-rider.md`,
`2026-05-18-2226-deadreckon-orchestration-eventbus-rider.md`,
`2026-05-28-1556-deadreckon-tamper-evident-gate-rider.md`) — their invariants
still apply. This rider adds a **campaign** layer: a meta-coordinator that spawns N
independent full orchestrators one level down, composes their merged results into a
single promoted run, and rolls their §35 gate verdicts up to the top.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime `~/.deadreckon/`.

## The idea in one paragraph (read before designing)

deadreckon already has the whole machine. `fork_command` (`main.rs:13217`) launches
N child *runs* as `deadreckon run` subprocesses, each isolated by
`DEADRECKON_SCOPE_ROOT` to a per-task launch dir (`run_plan_child`,
`main.rs:15934–15984`). `merge_command` (`main.rs:16362`) composes those children's
**library artifacts** into one promoted run (`compose_plan_merge_working`,
`main.rs:16698`), and that merged run is a *normal run* resolvable by id
(`resolve_plan_result_run`, `main.rs:13733`; AS-BUILT §30.4). **Campaign is the same
pattern lifted one level:** the meta-coordinator launches N `orchestrate full-plan`
subprocesses instead of N `run` subprocesses; each sub-orchestrator does its own
plan→fork→merge and produces a normal merged result run; the meta-merge composes
those N result runs with the *same* compose primitive. Nothing in the spawn,
isolation, or merge substrate needs reinventing — the work is the **meta layer and
its guardrails**.

## Posture (decided — do not redesign)

- **Production-release track.** Release-grade orchestration, not a toy.
- **Files-not-fields.** New durable state is files under
  `~/.deadreckon/plans/<campaign-id>/`: `campaign.json`, `campaign-coordinator.json`,
  `lineage.json`, `campaign-rollup.json`, `campaign-events.jsonl`,
  sub-plan launch sidecars. **No** new fields on `Plan`, `PlanTask`,
  `PipelineState`, `AcceptanceMarker`, or provider config. Sub-orchestrators
  produce ordinary `Plan`s and ordinary merged runs that existing code already
  handles.
- **Depth hard-capped at 2.** A campaign is depth 0; the orchestrators it spawns are
  depth 1; those orchestrators **must refuse** to fan out again (would be depth 2).
  The cap is a constant `CAMPAIGN_MAX_DEPTH: u32 = 2` — not configurable in this goal.
- **Independent sub-goals only.** Sub-goals carry **no cross-sub dependencies** in
  this goal (each sub-orchestrator is a parallel island). Cross-level dependency
  edges are a V1 candidate.
- **Reuse, don't fork.** Factor shared logic out of `compose_plan_merge_working` and
  `run_plan_child` rather than copying them. New code is the meta layer only.
- **Trust composes downward.** Every leaf run still goes through its own §35
  tamper-evident gate unchanged. The campaign adds *aggregation*, never a bypass.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Depth>2, cross-level merge-repair, recursive live attach,
  provider-planned dependency graphs across levels → `docs/V1-CANDIDATES.md`.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Vocabulary (use exactly)

- **campaign** — the meta operation / its plan file (`campaign.json`). The
  user-facing verb is the **top-level** `deadreckon campaign`, a peer to
  `run`/`orchestrate`/`chain` — *not* a subcommand of `orchestrate`.
- **meta-coordinator** — the process running `campaign`; owns
  `campaign-coordinator.json` while live.
- **sub-orchestrator** — a depth-1 `orchestrate full-plan` subprocess the
  meta-coordinator launches; owns its own normal `Plan` + coordinator.
- **leaf run** — any normal `deadreckon run` at the bottom (a sub-orchestrator's
  child run, or its merge/repair run). Leaves are where §35 verdicts are produced.
- **campaign result run** — the single promoted run the meta-merge creates.

## Data model (files, not fields)

### `~/.deadreckon/plans/<campaign-id>/campaign.json`

```jsonc
{
  "schema_version": 1,
  "campaign_id": "uuid-simple",
  "root_goal": "…",
  "n": 3,
  "depth": 0,                         // always 0 for a real campaign; guard refuses >0
  "providers": { "planner": "…", "default_child": "…" },  // reuse PlanProviders shape, serialized inline
  "sub_goals": [
    { "sub_id": "sub-0", "goal": "…", "task_key": "<derived>", "sub_plan_id": null,
      "result_run_id": null, "scope": null, "status": "pending" }
  ],
  "tree_budget_usd": 15.0,            // null if no --max-spend
  "tree_wall_seconds": null,
  "status": "pending",               // pending|forked|merged|failed|killed
  "merged_run_id": null,
  "created_at": "RFC3339",
  "forked_at": null,
  "merged_at": null,
  "deadreckon_version": "…"
}
```

`sub_goals[].status` ∈ `pending|running|merged|failed|killed`. `task_key` is derived
from the sub-goal via `paths::task_key` so the cycle guard can compare it to
ancestors.

### `~/.deadreckon/plans/<plan-id>/lineage.json` (written for every plan AND campaign)

```jsonc
{
  "schema_version": 1,
  "depth": 1,                         // 0 = top-level, 1 = spawned by a campaign
  "campaign_root_id": "…",              // the originating campaign_id (self if depth 0 campaign)
  "ancestor_task_keys": ["<root>", "<sub-0>"],   // for cycle detection
  "ancestor_scopes": ["repo-abc123"]
}
```

`lineage.json` is the **durable** record; the env vars below are the **transport**
that lets a freshly spawned subprocess know its depth before it has a plan dir.

### `~/.deadreckon/plans/<campaign-id>/campaign-rollup.json`

```jsonc
{
  "schema_version": 1,
  "campaign_id": "…",
  "evaluated_at": "RFC3339",
  "leaves": [
    { "sub_id": "sub-0", "result_run_id": "…", "leaf_run_ids": ["…"],
      "gate": "signed" | "refused" | "missing",
      "tamper_verdict": "clean" | "caveat" | "refuse",
      "caveats": ["…"] }
  ],
  "rollup_verdict": "clean" | "caveat" | "refused",  // worst-of across leaves
  "refused_subs": ["sub-2"],
  "caveat_subs": ["sub-1"]
}
```

`rollup_verdict` = worst of all leaves (`refused` > `caveat` > `clean`), reading each
leaf's `proofs/acceptance-tamper.json` (§35) and acceptance marker presence.

### `~/.deadreckon/plans/<campaign-id>/campaign-events.jsonl`

Append-only timeline mirroring `plan-events.jsonl`: `campaign_created`,
`campaign_started`, `sub_launched{sub_id}`, `sub_plan_discovered{sub_id, sub_plan_id}`,
`sub_merged{sub_id, result_run_id}`, `sub_failed{sub_id, reason}`,
`budget_exhausted`, `meta_merge_started`, `meta_merge_completed{merged_run_id}`,
`rollup_refused{refused_subs}`, `campaign_completed`, `campaign_failed`, `campaign_killed`.

## Env transport for lineage (spawn boundary)

When the meta-coordinator spawns a sub-orchestrator (generalized `run_plan_child`),
it sets, in addition to today's `DEADRECKON_HOME` / `DEADRECKON_SCOPE_ROOT`:

```
DEADRECKON_CAMPAIGN_DEPTH=1
DEADRECKON_CAMPAIGN_ROOT=<campaign-id>
DEADRECKON_CAMPAIGN_ANCESTOR_TASK_KEYS=<comma-joined>
DEADRECKON_CAMPAIGN_ANCESTOR_SCOPES=<comma-joined>
```

`orchestrate_command` and `plan_command` read these at startup, write `lineage.json`
into the new plan dir, and the depth/cycle guard (below) runs before any planner
call or child launch. Absent env => depth 0.

## Guard (the spec — P1)

`campaign::guard(depth_env, ancestors, requested_sub_keys, requested_scopes) -> Result<()>`:

1. **Depth.** If `DEADRECKON_CAMPAIGN_DEPTH >= CAMPAIGN_MAX_DEPTH` (i.e. a campaign is
   requested at depth ≥ 1, or a depth-1 orchestrator would itself fan out), **refuse**
   with `DeadreckonError::InvalidInput("campaign refused: depth cap 2 reached …")`.
   No `campaign.json` is written.
2. **Cycle.** If any requested sub-goal `task_key` ∈ `ancestor_task_keys`, or any
   resolved sub scope ∈ `ancestor_scopes`, **refuse** naming the colliding sub.
3. Otherwise `Ok(())`.

The depth-1 orchestrator enforces the cap by checking, inside
`orchestrate_command`/the new `campaign` subcommand, that
`DEADRECKON_CAMPAIGN_DEPTH == 0` before allowing `campaign`; a depth-1 process running
plain `orchestrate full-plan` is fine (that is the normal sub-orchestrator), but a
depth-1 process invoking `campaign` is the refused case.

## Tree budget allocator (P5)

`campaign::allocate_budget(tree_budget, n) -> Vec<f64>` splits `--max-spend` evenly
across the N sub-orchestrators (floor-divided, remainder to `sub-0`). Each
sub-orchestrator is launched with `--max-spend <share>`. The meta-coordinator also
tracks **aggregate** spend by summing every leaf run's `total_spend_usd`
(`state.total_spend_usd` per leaf, discovered via the sub-plan's child + merge run
ids) into `campaign.json`; before launching the next ready sub (when N>concurrency or
a sub is retried) it refuses with `budget_exhausted` if the aggregate ≥
`tree_budget_usd`. High tree budgets reuse the existing high-spend confirmation
prompt. A `null` tree budget means each sub inherits the default per-run cap (log
this — no silent unbounded campaign).

## Spawn (generalize `run_plan_child` — P3)

Factor a `SubOrchestratorLaunch` sibling of `PlanChildLaunch` (`main.rs:15920`).
Reuse the exact env-isolation idiom from `run_plan_child:15959–15964`, but the
subprocess argv is:

```
<current_exe> orchestrate full-plan <sub-goal>
    --from <source_dir> --n <sub_n> --yes --no-confirm --no-hints --no-docs
    --max-spend <share> [--plain] [--sandbox <s>]
```

`--sub_n` for each sub-orchestrator defaults to a fixed small value (e.g. 2) in this
goal; do not let the planner pick per-sub breadth here (V1). After the subprocess
returns, discover its plan id and merged result run id from
`plans/<campaign-id>/launch/<sub-id>/` sidecars (mirror the run-id sidecar pattern at
`main.rs` fork: "records the run id in `plans/<plan-id>/launch/<task-id>/run-id`",
§30.3). Record `sub_plan_id` and `result_run_id` into `campaign.json` and emit events.

Concurrency: reuse the existing `tokio::task::spawn_blocking` batch idiom from
`fork_command`. Cap concurrent sub-orchestrators at the same limit `fork` uses.

## Meta-merge (refactor + reuse — P6)

Extract the conflict-detection + worktree-compose core of
`compose_plan_merge_working` (`main.rs:16698`) into a reusable
`compose_result_runs(run_ids: &[String], strategy) -> ComposeOutcome` that operates
on a list of **promoted result-run library dirs**. Then:

- `merge_command` calls it with the plan's `child_run_id`s (behavior unchanged —
  guard with the existing plan-merge depth tests).
- the campaign meta-merge calls it with the sub-goals' `result_run_id`s.

Sub-goals are independent in this goal, so the meta-merge uses the `dag-aware`
default with **no cross-sub dependency edges**; genuine same-file conflicts across
two sub-results write `merge-proofs/conflicts.json` under the campaign dir and **fail
the campaign** (no auto-repair across levels in this goal — V1). The composed tree
is promoted as the **campaign result run** via the normal promotion path, and gets a
`deadreckon-campaign-manifest.json` (analog of `deadreckon-plan-manifest.json`, §30.4)
recording campaign id, root goal, N, sub ids, sub plan ids, leaf result run ids, and
the roll-up verdict.

## Gate-verdict roll-up (the trust spine — P7)

`campaign::rollup(campaign) -> CampaignRollup` walks every leaf run of every sub
(sub-orchestrator child runs + sub merge run + the sub's own result run), reads each
`proofs/acceptance-tamper.json` (§35) and acceptance marker, and computes
`rollup_verdict` = worst-of. Then:

- **Any leaf `refuse` (or missing marker on a required leaf)** => the campaign result
  run is **not** promoted to a clean done: emit `rollup_refused`, set
  `campaign.status = failed`, and surface the offending subs. The meta result must
  **never** reach a clean `Completed` while a leaf is refused — this is the
  invariant that stops nesting from laundering a hollow pass.
- **Any leaf `caveat`** => the campaign completes but the meta summary and
  `campaign-rollup.json` carry the caveat with `Warn` tone (reuse §35 render).
- **All clean** => normal clean completion.

The campaign result run's *own* acceptance (P8) is "every sub merged AND
`rollup_verdict != refused`"; bind `campaign-rollup.json`'s digest into the result
run's marker signature the same way §35 binds the tamper file, so the roll-up cannot
be edited after the fact without invalidating the result.

## CLI (P9) — `crates/deadreckon/src/cli.rs`

Add `campaign` as a **top-level command**, a peer of `Run`, `Orchestrate`, and
`Chain` in the root `Command` enum (cli.rs, alongside `Orchestrate` at cli.rs:774
and `Chain` at cli.rs:948) — **not** a variant of `OrchestrateCommand`. Per the
production command model (§17, AS-BUILT) it is an advanced verb: discoverable via
`deadreckon help-all`, per-command help, and completions, but not in the
five-to-live-by first screen. The `CampaignArgs` struct mirrors
`OrchestrateFullPlanArgs` (cli.rs:1688); reuse the orchestrate preflight and
summary renderers rather than copying them.

```
deadreckon campaign <goal>
    --n <2..=6>                 # number of sub-orchestrators (validate_task_count reuse)
    [--planner <provider>]      # planner route for sub-goal decomposition
    [--from <dir>] [--sandbox <s>] [--max-spend <usd>] [--max-wall-seconds <s>]
    [--yes] [--preview] [--plain] [--json] [--quiet]
```

Preflight (reuse the orchestrate preflight renderer): mode=`campaign`, N, **depth cap
2**, **tree budget** and per-sub share, planner/default-child provider roles,
sandbox, caps, and the N sub-goal rows. `--preview` writes `campaign.json` and stops
before launching. `--yes` is required for headless. Validate `--n` via the existing
`validate_task_count` (2..=6, plan.rs:420).

## Surfacing (P10) — `crates/deadreckon/src/main.rs`

- `attach <campaign-id>`: TTY shows N sub-plan rows (sub-goal, sub_plan_id, status,
  result run prefix, leaf spend, roll-up state) with a `campaign:<root-goal>`
  breadcrumb; `Enter` drills into the selected **sub-plan** using the existing plan
  attach view (one extra breadcrumb hop). Off-TTY prints a plain summary. **No**
  recursive event-tree streaming in this goal — each sub-plan's own `attach` remains
  the way to see its children (state this in the breadcrumb hint). `PlanEventBus`
  is reused per sub-plan, not extended to a tree (V1).
- `kill <campaign-id>`: read `campaign-coordinator.json`, signal the meta-coordinator,
  then cascade — for each `sub_plan_id` reuse the existing plan-kill path
  (`kill_command`, `main.rs:23138`) to stop each sub-coordinator and its children,
  then mark campaign killed. Cascade order: leaves → subs → meta.
- `show <campaign-id> --why-failed`: report refused/caveat subs from
  `campaign-rollup.json`, failed sub-orchestrators, budget exhaustion, and meta-merge
  conflicts.
- `status` / `list`: a campaign appears with `mode: campaign`, N subs, and the
  roll-up verdict (clean/caveat/refused) one line.

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them fail; implement;
green on `cargo test -p <touched crate>` for touched modules plus `cargo fmt
--check`; conventional-commit local commit; one-line CHANGELOG entry. Do not run
`make verify`/release/stress/full-workspace suites unless the human asks. Smokes use
the `smoke` provider with `--sandbox none`.

### P1 — Lineage + depth + cycle guard (RED)

- New core module `crates/deadreckon-core/src/campaign.rs` (re-export from `lib.rs`):
  `CampaignPlan`, `Lineage`, `CAMPAIGN_MAX_DEPTH`, `guard`, `read_lineage`,
  `write_lineage`. No spawning yet — just the guard and files.

Depth tests (`crates/deadreckon-core/src/campaign.rs`):
- `campaign_at_depth_one_is_refused`
- `subgoal_cycling_to_ancestor_task_key_is_refused`
- `subgoal_cycling_to_ancestor_scope_is_refused`
- `lineage_round_trips_and_defaults_depth_zero_when_absent`

### P2 — `campaign.json` model + sub-goal decomposition

- `CampaignPlan::new`, validation (N via `validate_task_count`, exactly-N planner
  output, non-empty distinct sub-goals). Planner decomposition reuses the full-plan
  planner contract (§30.1) but emits **sub-goals**, not single-run tasks.

Depth tests:
- `campaign_plan_rejects_n_outside_2_6`
- `campaign_plan_rejects_planner_count_mismatch`
- `campaign_plan_rejects_duplicate_subgoals`

### P3 — Sub-orchestrator spawn

- `SubOrchestratorLaunch` + `run_sub_orchestrator`, reusing the env-isolation idiom;
  argv launches `orchestrate full-plan … --yes --no-docs`; injects lineage env;
  discovers sub plan id + result run id from launch sidecars.

Depth tests (`crates/deadreckon/`):
- `sub_orchestrator_launch_sets_lineage_env_and_isolated_scope`
- `sub_orchestrator_result_run_is_discovered_from_launch_sidecar`

### P4 — Concurrent fork of N sub-orchestrators

- Batch-launch ready subs (reuse `fork_command`'s spawn_blocking idiom + concurrency
  cap); write `campaign-events.jsonl`; update `campaign.json` per sub transition.

Depth tests:
- `campaign_fork_launches_all_subs_and_records_events`
- `campaign_fork_marks_failed_sub_without_aborting_siblings`

### P5 — Tree budget allocator + aggregate enforcement

- `allocate_budget`; aggregate leaf-spend accounting; refuse-on-exhaustion;
  null-budget warning.

Depth tests:
- `tree_budget_splits_evenly_with_remainder_to_first`
- `aggregate_spend_exhaustion_refuses_next_sub_launch`
- `null_tree_budget_logs_unbounded_warning`

### P6 — Meta-merge via shared compose

- Extract `compose_result_runs`; rewire `merge_command` onto it (unchanged
  behavior); meta-merge composes sub result runs; cross-sub conflict fails the
  campaign; promote campaign result run + `deadreckon-campaign-manifest.json`.

Depth tests:
- `compose_result_runs_extracted_without_changing_plan_merge`
- `campaign_meta_merge_composes_two_clean_sub_results`
- `cross_sub_file_conflict_fails_campaign`

### P7 — Gate-verdict roll-up

- `rollup`; `campaign-rollup.json`; any refused leaf fails the campaign; caveat
  surfaces; bind roll-up digest into the result-run marker signature.

Depth tests:
- `refused_leaf_makes_campaign_fail_and_blocks_clean_completion`
- `caveat_leaf_surfaces_caveat_but_campaign_completes`
- `all_clean_leaves_yield_clean_rollup`
- `edited_rollup_file_fails_result_marker_signature`

### P8 — Campaign acceptance / completion

- Define campaign "done" = all subs merged AND `rollup_verdict != refused`; wire the
  result-run gate to require it. Reuse the non-terminal failure idiom on refusal.

Depth tests:
- `campaign_completes_only_when_all_subs_merged_and_no_refusal`
- `campaign_with_refused_sub_never_reaches_completed`

### P9 — CLI verb + preflight

- top-level `campaign` command (peer to run/orchestrate/chain) + `CampaignArgs`; preflight render; `--preview`
  stops after `campaign.json`; `--yes` headless; `validate_task_count` on `--n`.

Depth tests:
- `fan_out_preview_writes_campaign_json_and_stops`
- `fan_out_preflight_shows_depth_cap_and_tree_budget`
- `fan_out_rejects_n_outside_range_at_cli`

### P10 — attach / kill / why-failed / status surfacing

- Campaign attach rows + breadcrumb + sub drill-in; `kill` cascade leaves→subs→meta;
  `show --why-failed`; `status`/`list` rows.

Depth tests:
- `campaign_attach_lists_subs_with_rollup_and_breadcrumb`
- `kill_campaign_cascades_to_sub_coordinators_and_children`
- `why_failed_reports_refused_and_caveat_subs`

### P11 — AS-BUILT + CHANGELOG + V1-CANDIDATES (doc only; no depth test)

- New AS-BUILT section:
  ```
  ## 36. Campaign Orchestration (one task, N orchestrators)

  36.1 Mental model: fork→merge lifted one level
  36.2 Files: campaign.json, lineage.json, campaign-rollup.json, campaign-events.jsonl
  36.3 Depth cap and cycle guard (CAMPAIGN_MAX_DEPTH = 2)
  36.4 Lineage env transport across the spawn boundary
  36.5 Tree budget allocation and aggregate enforcement
  36.6 Sub-orchestrator spawn and result-run discovery
  36.7 Meta-merge via shared compose_result_runs
  36.8 Gate-verdict roll-up and the no-laundering invariant
  36.9 attach/kill/why-failed for campaigns
  36.10 Current limits (depth 2, independent subs, no recursive attach)
  ```
  Update §30 cross-references and the "shipped vs scaffolding-thin" list: add
  campaign (depth 2) to shipped; state plainly it does **not** support depth>2,
  cross-level dependencies, cross-level merge-repair, or recursive live attach.
- Append to `CHANGELOG.md`:
  ```
  ## Campaign Orchestration (production release) — 2026-05-28

  - Added top-level `deadreckon campaign <goal> --n <2..=6>` (peer to run/orchestrate/chain): one meta-coordinator
    spawns N independent full orchestrators (depth-capped at 2), composes their
    merged results into one promoted run, and rolls every leaf's tamper-evident
    gate verdict up to the top.
  - Tree-wide spend ceiling, cycle guard, and a no-laundering roll-up: a campaign
    cannot reach clean completion while any leaf run was refused.
  - attach/kill/why-failed/status understand campaigns.
  ```
- Log to `docs/V1-CANDIDATES.md`: depth>2 with cycle-safe recursion; cross-sub
  dependency edges and cross-level merge-repair; recursive live attach with a true
  event tree (extend `PlanEventBus` to a hierarchy); planner-chosen per-sub breadth;
  tree-budget strategies beyond even split.

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| `campaign refused: depth cap 2 reached` | run `orchestrate full-plan` (not campaign) inside a sub-orchestrator |
| `campaign refused: sub-goal 'X' cycles to ancestor` | reword the sub-goal so its task differs from an ancestor |
| `campaign failed: sub 'sub-2' refused (tamper)` | `deadreckon show <campaign-id> --why-failed`, then fix sub-2 and re-fan |
| `campaign paused: tree budget exhausted` | raise `--max-spend` or reduce `--n`, then re-run |
| `campaign failed: cross-sub file conflict` | narrow sub-goals so they touch disjoint files (cross-level repair is V1) |

(Each footer is exercised by a P1/P5/P6/P7/P10 depth test.)

## Integration matrix

| Verb | Plain run | Plan (existing) | Campaign (new) |
|---|---|---|---|
| spawn | n/a | `run_plan_child` → N runs | `run_sub_orchestrator` → N orchestrators |
| isolation | per-run scope | `DEADRECKON_SCOPE_ROOT` per task | same + lineage env |
| compose | n/a | `compose_plan_merge_working` | `compose_result_runs` (shared) |
| gate | per run (§35) | per child + merge run | per leaf + **roll-up** |
| attach | run TUI | plan TUI | campaign TUI → drill to plan TUI |
| kill | run | plan + children | meta + subs + children (cascade) |

## Out of scope (explicitly V1 candidates)

- Depth greater than 2 / arbitrary recursion.
- Cross-sub dependency edges and cross-level (meta) merge-repair.
- Recursive live attach with a hierarchical event tree (`PlanEventBus` stays
  single-plan; campaign attach is one drill-in hop).
- Planner-chosen per-sub breadth and per-sub provider roles.
- Tree-budget strategies beyond even split (weighted, dynamic reallocation).
- Sharing campaign records across machines.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (already in-tree): `uuid`, `serde`/`serde_json`, `chrono`, `tokio`,
`walkdir`. **No new crates expected.** Tier 2: none. Tier 3: same blocks as prior
riders.

## Engineering invariants (do not violate)

- **No `Plan`/`PlanTask`/`PipelineState`/`AcceptanceMarker`/provider field
  additions.** Campaign state is files under `plans/<campaign-id>/`.
- **`CAMPAIGN_MAX_DEPTH = 2` is a hard constant.** A depth-1 process invoking campaign
  refuses. Guarded by `campaign_at_depth_one_is_refused`.
- **The no-laundering invariant is sacred.** A campaign must never reach a clean
  `Completed` while any leaf run was refused. Guarded by
  `refused_leaf_makes_campaign_fail_and_blocks_clean_completion` and
  `campaign_with_refused_sub_never_reaches_completed`.
- **`compose_plan_merge_working` behavior is unchanged** after the
  `compose_result_runs` extraction — the existing plan-merge depth tests must stay
  green (`compose_result_runs_extracted_without_changing_plan_merge`).
- **One depth test before each phase implementation.** P1's guard tests prove the
  refusal cases before any spawning exists.
- **No silent unbounded campaign.** A null tree budget is logged, never assumed
  infinite per leaf.
- **No silent expansion.** Anything beyond P1–P11 goes to `V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth tests passing, `cargo fmt --check` clean, and a
  CHANGELOG entry naming the SHA.
- If a phase reveals a V1-architecture decision (e.g. cross-level merge-repair is
  unavoidable for a real sub-goal set), stop and log it in `V1-CANDIDATES.md`; do not
  silently expand scope past depth 2.
- Optional after P11: an asciinema cast of a 2-sub campaign (one clean, one caveat)
  under `/Users/gdc/deadreckon/` demo assets. Skip if not worth it.
