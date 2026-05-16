# deadreckon - Semantic Merge Repair Rider (DAG-aware + planner-mediated integration)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-16-1122-deadreckon-semantic-merge-repair-goal.md`.
It supersedes nothing in prior riders
(`2026-05-11-1444-deadreckon-orchestrate-rider.md`,
`2026-05-15-2252-deadreckon-plan-events-rider.md`,
`2026-05-13-1900-deadreckon-coherence-rider.md`) - their invariants still
apply. This rider adds semantic merge repair for orchestration plans: DAG-aware
file precedence first, structured conflict bundles second, planner-mediated
repair only when deterministic merge cannot prove the right answer.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided - do not redesign)

- **Maturity stays `alpha`.**
- **No `PipelineState` schema changes.** Repair runs are normal runs; repair
  metadata is plan-local sidecar state under `merge-proofs/`.
- **Avoid `Plan` schema expansion.** Prefer `plan-events.jsonl`,
  `merge-proofs/*.json`, worker specs, summaries, and child run markers.
- **Keep child runs normal.** No new turn loop, no special provider adapter, no
  hidden child-to-child chat.
- **Automatic, bounded repair before manual work.** Deterministic DAG
  precedence is first. Planner-mediated repair is automatic for `merge` and
  `orchestrate`, but bounded to plan-local `merge-working`, named conflict
  paths, one repair attempt by default, and durable rationale. Users can opt
  out with `--no-repair`.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Rich web merge UIs, three-way AST merge engines,
  cross-repo orchestration, and always-on autonomous repair policy go to
  `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Motivation To Capture In Tests

Plan `aa20e56506a548d8ba4bece648c55b26` completed all four children for a
flight simulator goal, then failed at merge:

```text
merge conflict at src/entities/airplane.js between child 0 and child 1
```

The graph was linear:

```text
task-0 -> task-1 -> task-2 -> task-3
```

So the first fix is not "ask the user to prefer child 1"; it is "the merge
engine should know that a descendant task intentionally extends an ancestor's
file." Planner repair is for unresolved semantic conflicts after deterministic
graph rules have done their work.

## Current Code Surfaces

- `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs`
  - `merge_command`
  - `compose_plan_merge_working`
  - `create_merged_plan_run`
  - `child_artifact_root`
  - `plan_task_depends_on`
  - `plan_child_source_dir`
- `/Users/gdc/deadreckon/crates/deadreckon-core/src/plan.rs`
  - `Plan`, `PlanTask`, `PlanEventKind`
  - `append_plan_event`, `read_plan_events`
  - `write_child_summary`
- `/Users/gdc/deadreckon/crates/deadreckon/tests/orchestrate.rs`
  - existing plan/fork/merge integration coverage
  - source-chaining regressions near `fork_respects_task_dependencies`

## Data Model (files, not fields)

### `merge-proofs/conflicts.json`

The existing conflict file may grow from the current compact shape, but keep it
versioned and backward-tolerant:

```json
{
  "schema_version": 2,
  "plan_id": "<plan-id>",
  "strategy": "fail-on-conflict|prefer-child|dag-aware|repair",
  "conflicts": [
    {
      "path": "src/entities/airplane.js",
      "children": [
        {
          "task_id": "task-0",
          "task_index": 0,
          "run_id": "<run-id>",
          "artifact_root": "/Users/gdc/.deadreckon/library/.../task-0",
          "hash": "<content hash>",
          "depends_on": []
        },
        {
          "task_id": "task-1",
          "task_index": 1,
          "run_id": "<run-id>",
          "artifact_root": "/Users/gdc/.deadreckon/library/.../task-1",
          "hash": "<content hash>",
          "depends_on": ["task-0"]
        }
      ],
      "deterministic_resolution": {
        "kind": "descendant_overrides_ancestor|identical|none",
        "chosen_task_id": "task-1",
        "reason": "task-1 depends on task-0 and changed the same file"
      }
    }
  ]
}
```

### `merge-proofs/repair-request.json`

Written before invoking a planner for semantic repair:

```json
{
  "schema_version": 1,
  "plan_id": "<plan-id>",
  "root_goal": "<root goal>",
  "provider": "cli:codex",
  "created_at": "<RFC3339>",
  "merge_working": "/Users/gdc/.deadreckon/plans/<plan-id>/merge-working",
  "conflicts": [
    {
      "path": "src/entities/airplane.js",
      "versions": [
        {
          "task_id": "task-0",
          "subject": "Scaffold project + 3D world",
          "summary_path": "/Users/gdc/.deadreckon/plans/<plan-id>/summaries/task-0.md",
          "artifact_path": "/Users/gdc/.deadreckon/library/.../src/entities/airplane.js"
        }
      ]
    }
  ]
}
```

### `merge-proofs/repair-plan.json`

Planner output after validation:

```json
{
  "schema_version": 1,
  "plan_id": "<plan-id>",
  "decision": "prefer_child|synthesize|spawn_repair_child|refuse",
  "rationale": "task-1 is a descendant of task-0 and adds controls",
  "actions": [
    {
      "path": "src/entities/airplane.js",
      "action": "prefer_child|write_synthesized|repair_child",
      "chosen_task_id": "task-1",
      "preserve": ["placeholder mesh", "control surfaces", "flight model update signature"]
    }
  ],
  "repair_goal": "Resolve merge conflicts preserving descendant behavior and root acceptance."
}
```

### `merge-proofs/repair-run.json`

Written only when a repair child is executed:

```json
{
  "schema_version": 1,
  "plan_id": "<plan-id>",
  "run_id": "<repair-run-id>",
  "scope": "<scope>",
  "status": "completed|failed|killed",
  "source": "/Users/gdc/.deadreckon/plans/<plan-id>/merge-working",
  "created_at": "<RFC3339>",
  "updated_at": "<RFC3339>"
}
```

## Merge Algorithm

### Deterministic pass

`compose_plan_merge_working` must become graph-aware:

1. Iterate completed tasks in task index order.
2. Skip generated artifacts with `skip_plan_merge_file`.
3. If a file has not been seen, copy it.
4. If the new content hash equals the seen hash, accept silently.
5. If the new owner depends on the previous owner, copy the new file and mark
   `descendant_overrides_ancestor`.
6. If the previous owner depends on the new owner, keep the previous file and
   mark `ancestor_candidate_skipped`.
7. Otherwise record a true conflict.

This mirrors dependency source composition. A linear chain should not fail
because later tasks touched earlier files.

### Automatic repair pass

If true conflicts remain:

1. Write `conflicts.json` and `repair-request.json`.
2. If repair is disabled with `--no-repair`, refuse with a clear conflict
   report and artifact paths.
3. Resolve a repair provider. If none is available, refuse with a provider
   setup hint; do not ask the user to run a second merge command.
4. Invoke planner provider with a read-only prompt and the repair request.
5. Validate planner JSON. Refuse malformed or unsafe decisions.
6. For `prefer_child`, copy the chosen child version and append rationale.
7. For `synthesize`, write only the named conflict paths from planner output.
8. For `spawn_repair_child`, run a normal `deadreckon run` from `merge-working`
   with a repair goal and the conflict bundle, then retry merge.
9. For `refuse`, append a repair failure event and print the rationale.
10. Stop after the configured repair attempt cap; default cap is one repair
    planner decision plus one repair child run if chosen.

Repair must never delete child libraries. It only mutates `merge-working` or a
repair run's working directory.

## Planner Prompt Contract

The repair planner is not a general coding agent. It returns JSON only:

```json
{
  "decision": "prefer_child|synthesize|spawn_repair_child|refuse",
  "rationale": "<short reason>",
  "actions": [
    {
      "path": "<relative conflict path>",
      "action": "prefer_child|write_synthesized|repair_child",
      "chosen_task_id": "<task-id-or-null>",
      "content": "<only for write_synthesized>",
      "preserve": ["<semantic requirements>"]
    }
  ],
  "repair_goal": "<only for spawn_repair_child>"
}
```

Prompt inputs:

- root goal
- plan id and mode
- task graph with `depends_on`
- provider roles
- worker specs for conflicting tasks
- child summaries
- conflict paths
- child artifact paths and short diffs where available
- current `merge-working` path

Prompt prohibitions:

- Do not invent new tasks unrelated to conflicts.
- Do not inspect sibling transcripts.
- Do not choose a parallel child without explaining why the other version is
  obsolete or fully subsumed.
- Do not write files during the planner step.

## CLI Surface

Add flags to `merge`:

```text
deadreckon merge <plan-id>
    [--strategy fail-on-conflict|prefer-child|dag-aware]
    [--prefer-child <idx>]
    [--no-repair]
    [--repair-provider <provider>]
    [--repair-mode prefer|synthesize|child|auto]
    [--repair-attempts <n>]
    [--yes]
```

Semantics:

- Default `merge` uses deterministic DAG-aware merge, then automatically runs
  planner-mediated repair for true conflicts.
- `--no-repair` restores raw conflict refusal for debugging or audit-only use.
- `--repair-mode prefer` permits only planner-chosen file preference.
- `--repair-mode synthesize` permits direct synthesized conflict-path writes.
- `--repair-mode child` permits only a repair run from `merge-working`.
- `--repair-mode auto` is the default and permits all validated repair
  decisions.
- `--repair-provider` defaults to `plan.providers.planner`, then
  `plan.providers.default_child`, then configured default provider.
- `--repair-attempts` defaults to `1`; `0` is equivalent to `--no-repair`.
- `--yes` keeps existing start confirmation behavior but is not required to
  make repair automatic once the merge command is already running.

`orchestrate` behavior:

- One-command orchestration runs semantic repair automatically after merge
  conflict, in both interactive and headless `--yes` mode.
- The started banner should say repair is automatic and name the provider that
  will be used if merge conflicts remain.
- If repair cannot safely proceed, the final output explains the failed repair
  rationale and points at `merge-proofs/`, not a manual command the user must
  discover.

## Plan Events

Add variants to `PlanEventKind`:

```rust
MergeRepairPlanned { conflict_count: usize, provider: Option<String> },
MergeRepairStarted { mode: String },
MergeRepairRunDiscovered { run_id: String, pid: Option<u32> },
MergeRepaired { strategy: String, repair_run_id: Option<String> },
MergeRepairFailed { reason: String },
```

Names may adjust for Rust style, but JSON tags must be snake_case. Existing
events remain valid.

## Refusal Cases

| Case | Behavior | `try:` |
|---|---|---|
| True conflict and `--no-repair` | Stop after writing conflict bundle | rerun without `--no-repair` |
| True conflict and no repair provider | Record `merge_repair_failed`; leave `merge-working` intact | `deadreckon providers list --all` |
| Planner JSON malformed | Record `merge_repair_failed`; leave `merge-working` intact | rerun with another provider |
| Planner chooses unknown task id | Refuse before file writes | inspect `merge-proofs/repair-plan.json` |
| Planner writes path outside conflict set | Refuse before file writes | use repair child mode |
| `--repair-mode prefer` but planner wants synthesize | Refuse with rationale | rerun with `--repair-mode synthesize` |
| Repair child fails gate | Plan remains forked/failed with repair-run pointer | `deadreckon attach <repair-run-id>` |
| Merge retry still conflicts | Record repair failed and show remaining paths | inspect `conflicts.json` |

Every refusal must be depth-tested or covered by a parameterized test.

## Phases (eleven)

Each phase: write the named depth test(s) first and watch them fail; implement;
green on focused tests for the touched surface and, at milestone boundaries,
`cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`;
conventional local commit; one-line CHANGELOG entry.

### P1 - Conflict model and fixture hardening

- Version `conflicts.json` without breaking old conflict readers.
- Add test helpers that create plan children with controlled overlapping files.
- Keep fixtures tiny; do not depend on real `aa20e565` runtime state.

Depth tests:

- `merge_conflict_bundle_records_all_child_versions`
- `merge_conflict_bundle_is_backward_tolerant`
- `merge_conflict_bundle_skips_generated_artifacts`

### P2 - DAG-aware deterministic merge

- Teach `compose_plan_merge_working` to use `plan_task_depends_on`.
- Descendant file changes override ancestor file changes.
- Ancestor candidates do not override descendants.
- Parallel conflicts still fail.

Depth tests:

- `merge_allows_descendant_child_to_override_ancestor_file`
- `merge_keeps_descendant_when_ancestor_seen_later`
- `merge_parallel_children_still_conflict`

### P3 - Automatic repair CLI flags and refusal UX

- Add `--no-repair`, `--repair-provider`, `--repair-mode`,
  `--repair-attempts`, and `--yes` to merge.
- Default merge remains deterministic first, then repair-capable.
- True conflict writes repair request and starts repair automatically unless
  disabled.

Depth tests:

- `merge_conflict_starts_repair_by_default`
- `merge_no_repair_prints_conflict_without_planner`
- `merge_auto_repair_resolves_provider_from_plan_then_config`
- `merge_repair_mode_refuses_unsupported_decision`

### P4 - Repair request writer

- Write `merge-proofs/repair-request.json` with root goal, graph, conflicts,
  child summary paths, worker spec paths, artifact paths, and merge-working.
- Include enough context for a planner to decide without reading transcripts.

Depth tests:

- `merge_repair_request_includes_task_graph_and_summaries`
- `merge_repair_request_includes_conflicting_artifact_paths`
- `merge_repair_request_never_points_outside_plan_or_library_roots`

### P5 - Repair planner invocation and validation

- Add a read-only planner prompt.
- Parse and validate planner JSON.
- Refuse malformed JSON, unknown paths, unknown tasks, and decisions outside
  `--repair-mode`.

Depth tests:

- `merge_repair_planner_json_roundtrips`
- `merge_repair_rejects_unknown_conflict_path`
- `merge_repair_rejects_unknown_task_id`
- `merge_repair_rejects_malformed_planner_response`

### P6 - Prefer-child repair decisions

- Implement validated planner `prefer_child` decisions.
- Copy the chosen child version into `merge-working`.
- Record rationale in `repair-plan.json` and plan events.
- Retry merge after preference repair.

Depth tests:

- `merge_repair_prefer_child_records_rationale_and_promotes`
- `merge_repair_prefer_child_does_not_mutate_child_libraries`
- `show_why_failed_reports_prefer_child_repair_when_it_fails`

### P7 - Synthesized conflict-path writes

- Implement `write_synthesized` only for explicit conflict paths.
- Require planner content for each synthesized path.
- Refuse path traversal and new-file writes in this mode.

Depth tests:

- `merge_repair_synthesizes_only_conflict_paths`
- `merge_repair_synthesize_rejects_path_traversal`
- `merge_repair_synthesize_then_retries_gate_and_promotes`

### P8 - Repair child execution

- Implement `spawn_repair_child` as a normal `deadreckon run --from <merge-working>`.
- Pass a precise repair goal and the conflict bundle path.
- Write `repair-run.json`.
- Emit repair run discovery event.
- Retry merge from the repair artifact on success.

Depth tests:

- `merge_repair_child_runs_from_merge_working`
- `merge_repair_child_records_run_id_and_scope`
- `merge_repair_child_failure_preserves_conflict_bundle`
- `merge_repair_child_success_retries_merge_and_promotes`

### P9 - Attach/show/history integration

- Plan attach should surface repair status, latest repair event, repair run id,
  and remaining conflict paths.
- `show <plan-id> --why-failed` should explain whether deterministic merge,
  planner repair, repair child, or final gate failed.
- `history grep --plan` should include repair events.

Depth tests:

- `attach_plain_plan_shows_merge_repair_status`
- `show_why_failed_plan_names_repair_run_and_conflict_paths`
- `history_grep_plan_finds_merge_repair_events`

### P10 - Orchestrate wrapper behavior

- Interactive `orchestrate` should run repair automatically after merge
  conflict without asking the user for a second command.
- Headless `orchestrate --yes` should also auto-repair within the same command.
- Started/finished banners should name automatic repair posture, provider,
  plan paths, and repair artifacts.

Depth tests:

- `orchestrate_interactive_merge_conflict_auto_repairs`
- `orchestrate_headless_merge_conflict_auto_repairs_with_yes`
- `orchestrate_no_repair_prints_artifact_paths`

### P11 - Architecture doc update + CHANGELOG

- Update `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:
  - Section 30: describe DAG-aware merge and planner-mediated repair.
  - Section 32: add repair lifecycle events.
  - Section 22: mark raw conflict-only merge as closed if fully landed; list any
    remaining thin items honestly.
- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:

```markdown
## Semantic merge repair (alpha) - 2026-05-16

- Added DAG-aware orchestration merge so descendant task artifacts can
  supersede ancestor file versions without manual conflict strategy.
- Added structured merge repair bundles and planner-mediated repair for true
  cross-child conflicts.
- Added merge repair events and plan attach/show/history surfacing.
```

- Add any deferred major choices to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

Depth test: documentation review only; no new test required.

## Verification Matrix

Run focused tests per phase. Before final commit:

```text
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo build --release
```

Required smoke scenarios:

1. Linear graph, same-file edit:
   - task-0 writes `src/entities/airplane.js`
   - task-1 depends on task-0 and edits it
   - `merge <plan-id>` succeeds without `--prefer-child`

2. Parallel conflict:
   - task-0 and task-1 both write different `shared.js`
   - `merge <plan-id>` writes conflict bundle and starts repair automatically

3. Planner prefer repair:
   - fake planner chooses child 1 with rationale
   - merged artifact includes child 1 content and plan events include repair

4. Repair child:
   - fake planner asks for repair child
   - repair child writes resolved file
   - merge retry promotes a merged run

5. Refusal:
   - fake planner returns unsafe path
   - no file write occurs; plan records `merge_repair_failed`

## Stop Conditions

Stop when:

- all depth tests named above exist and pass;
- deterministic DAG merge and automatic repair both work;
- plan attach/show/history mention repair status;
- AS-BUILT and CHANGELOG are updated;
- no edits outside `/Users/gdc/deadreckon/`;
- no `PipelineState` schema changes;
- work is committed locally and not pushed.
