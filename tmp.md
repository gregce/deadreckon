# Run vs Orchestrate Process Flow and State Management

Current flow, based on `run_command`, `orchestrate_command`, `fork_command`, and the `Plan`/`PipelineState` models.

| Area | `deadreckon run` | `deadreckon orchestrate` |
|---|---|---|
| Primary unit | One run. | One plan plus multiple child runs, then usually one merged run. |
| Durable state object | `PipelineState` in `runstate/<scope>/runs/<run-id>/state.json`. | `Plan` in `plans/<plan-id>/plan.json`; each child is also a normal `PipelineState`. Merge creates another `PipelineState`. |
| Top-level flow | Resolve provider/config, ensure acceptance, resolve source mode, preview/confirm, create run, execute turn loop, promote/export/apply hints. | Prepare source, create plan, preview/confirm, `fork` child runs, then `merge` completed child artifacts. |
| Source setup | Chooses `worktree`, `copy`, `fresh`, or `in-place`; `copy` copies `--from` into run working dir. | Stores `parent_cwd` on the plan. Child runs use parent cwd, dependency artifacts, or reviewer `extend` depending on task graph/role. |
| Provider model | One execution provider for the run, plus optional doc provider. | Planner provider creates the graph; child/coder/reviewer providers execute tasks. Per-child overrides are stored in plan providers. |
| Planner state | No separate planner step; the run goal is the work item. | Planner call returns task JSON directly; it is not itself a `PipelineState`. The result becomes durable plan/tasks. |
| Execution loop | `run_turn_loop` mutates one working dir over turns, snapshots before/after, records traces/spend/provenance, gates, then promotes. | `fork` schedules ready tasks by dependencies, launches child `deadreckon run` or reviewer `deadreckon extend`, records child run ids/scopes/statuses back into plan. |
| Dependency handling | Continuation is via `resume`/`extend`, not a task DAG inside one run. | Plan task DAG controls readiness. Dependent full-plan children launch from composed dependency artifacts under `plans/<plan-id>/launch/<task-id>/source`. |
| Status model | Run status: `pending`, `planned`, `executing`, `completed`, `failed`, `killed`; phase ids track init/plan/provider/sandbox/execute/verify/complete. | Plan status: `pending`, `forked`, `merged`, `failed`; task status: `pending`, `running`, `completed`, `failed`, `killed`. Child runs still have normal run statuses. |
| Events/logs | Run-local `events.jsonl`, `traces.jsonl`, `spend.jsonl`, `provenance.jsonl`, snapshots, docs. | Plan-level `plan-events.jsonl`, `messages.jsonl`, worker specs, summaries, transient `coordinator.json`; child runs have their own normal run logs. |
| Live tracking | `child_pids` on run state and pid files for provider/tool calls. | Coordinator records live child pids; `TaskRunDiscovered` events connect plan tasks to child run ids/pids. |
| Cancellation | Kill targets the run and its recorded child/provider/tool processes. | Kill targets the plan and cascades into child runs/processes; child runs also retain normal kill semantics. |
| Completion artifact | Completed copy/fresh runs are promoted into library; worktree/in-place finish differently. | Each child can promote its own artifact. `merge` composes completed child artifacts into `merge-working`, creates a merged completed run, promotes that library, and stores `merged_run_id` on the plan. |
| Conflict handling | Within one working dir, conflicts are just normal file state. | Merge detects cross-child file conflicts. Default is fail-on-conflict; dependency source composition also fails before launching a dependent child on parallel conflicting files. |
| Attach/show mental model | Attach/show one run. | Attach/show the plan for orchestration-level progress; drill into child run ids for normal run-level detail. |

Short version: `run` is a single durable execution lane. `orchestrate` is a coordinator state machine that creates a durable task graph, runs each task as a regular `run`/`extend`, records their outputs back into the plan, then optionally creates one merged run from the child artifacts.
