# deadreckon — Usability Rider (materialize + extend)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-11-1400-deadreckon-usability-goal.md`. Prior
riders (`2026-05-10-1400-deadreckon-build-rider.md`,
`2026-05-11-1400-deadreckon-primary-flow-rider.md`,
`2026-05-11-1400-deadreckon-robust-rider.md`) still apply — their invariants,
dependency policy, UX commitments, sandbox defaults, and CLI surface hold.
This rider adds two CLI verbs (`materialize`, `extend`), supporting types,
lifecycle hints, and named tests.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided — do not redesign)

- **No schema changes.** `PipelineState` stays as it is. Parent lineage and
  materialization status live in **files** under the run's working tree:
  `working/.deadreckon/parent.json` (when extended) and
  `library/<scope>/<run-id>/.materialized-to` (when materialized).
- **No new V1 features.** This is two verbs plus integration polish.
- **Existing primitives are reused.** `copy_tree` (artifacts.rs:117), `create_run` (state.rs:178), `RunLoopConfig::from_turn`, `DeadreckonPaths`, the lock layer.

## Verb 1: `materialize`

### Signature

```
deadreckon materialize <run-id> [--dest <path>] [--force] [--include-manifest]
```

### Semantics

- **Source.** `library/<scope>/<run-id>/` (the promoted artifacts only; never
  the runstate tree).
- **Default dest.** `./<run-id-prefix>` where prefix is the first 8 chars of
  the run-id. Never the current working directory.
- **Refuse non-empty dest.** If `--dest` exists and is not empty, fail with
  `error: dest <path> is not empty (use --force to overwrite, or pass a
  fresh path)`. With `--force`, the dest is wiped and re-created.
- **File permissions.** 0644 for files, 0755 for directories. The dest is
  user-owned and editable.
- **Provenance marker.** Always write `.deadreckon/parent.json` in the dest
  (see schema below). `--include-manifest` additionally copies the
  promoted-library `manifest.json`.
- **Reverse marker.** Write `library/<scope>/<run-id>/.materialized-to`
  containing one line per materialization: ISO timestamp + `\t` + absolute
  dest path. Append-only; multiple materializations are recorded.

### `.deadreckon/parent.json` schema

```json
{
  "schema_version": 1,
  "kind": "materialized",
  "parent_run_id": "<run-id>",
  "parent_scope": "<scope>",
  "parent_goal": "<original goal>",
  "parent_completed_at": "<RFC3339>",
  "materialized_at": "<RFC3339>",
  "deadreckon_version": "<crate version>"
}
```

### Refusal cases

- Run not found → `error: no run <id>`.
- Run status not `Completed` → `error: run <id> is not completed (status=<x>); use 'deadreckon resume' first`.
- Source library dir missing → `error: library missing for run <id>; was promotion successful?`.
- Dest is inside `/Users/gdc/.deadreckon/` → `error: refusing to materialize back into runstate (pick a path outside ~/.deadreckon/)`.

## Verb 2: `extend`

### Signature

```
deadreckon extend <parent-run-id> "<new-goal>" \
  [--dest <path>] [--max-context-turns N] [--no-context] \
  [--max-spend <USD>] [--max-wall-seconds <N>] [--provider <id>] [--sandbox <kind>]
```

### Semantics

- **Parent must be `Completed`.** Otherwise refuse with `error: parent <id>
  is <status>; use 'deadreckon resume' for incomplete runs`.
- **New run-id.** Fresh UUID via `Uuid::new_v4()`.
- **Same scope and task_key as parent** by default. (Different `--dest` is
  fine for the working dir; the lock is task-scoped per `lock.rs:15–24`.)
- **Working dir seed.** Copy `library/<scope>/<parent-id>/` content into the
  new run's `working/`. Skip `manifest.json` and any `.materialized-to`
  marker.
- **History pre-population.** Build a `parent-summary.md` string (format
  below); prepend it to the new run's `history.json` as a single entry.
  Then emit a synthetic first `traces.jsonl` entry of kind
  `extended_from_parent` with the parent run-id, scope, parent goal,
  parent completion time.
- **`.deadreckon/parent.json` schema** (in new run's working dir):

  ```json
  {
    "schema_version": 1,
    "kind": "extended",
    "parent_run_id": "<old>",
    "parent_scope": "<scope>",
    "parent_goal": "<old goal>",
    "parent_completed_at": "<RFC3339>",
    "extended_at": "<RFC3339>",
    "new_goal": "<provided>",
    "context_turns_included": <N or null>,
    "deadreckon_version": "<crate version>"
  }
  ```

- **Resource caps reset.** `--max-spend` and `--max-wall-seconds` default to
  the same fresh-run defaults; do **not** inherit the parent's totals.
- **Lock acquisition.** Same task lock as the parent. If the parent's lock
  is held by another process, refuse with the standard "already running"
  error.

### Parent-summary format (`parent-summary.md`)

```markdown
# Previous run summary (<parent-run-id>)

**Original goal.** <parent_goal>
**Completed.** <RFC3339>
**Total turns.** <N>
**Total spend.** <USD or "subscription">
**Acceptance.** <acceptance kind + result line>

## Recent activity (last <max-context-turns> turns)

- turn <N-k>: <trace one-liner>
...
```

The last-N-turns block is omitted when `--no-context` is passed.
`--max-context-turns 0` is equivalent.

### Refusal cases

- Parent run not found → `error: no run <id>`.
- Parent status ≠ `Completed` → see above.
- Parent library missing → `error: parent library missing; cannot extend`.
- Lock held → `error: task already running (lock held by pid <X>)`.
- New goal is empty/whitespace → `error: --goal must be non-empty`.

## CLI surface additions

```
deadreckon materialize <run-id> [--dest <path>] [--force] [--include-manifest]
deadreckon extend <parent-run-id> "<new-goal>" [...flags above]
```

Both are added to `Commands` in `crates/deadreckon/src/main.rs:90`.

`deadreckon --help` lists them under "Lifecycle" alongside `run`, `resume`,
`undo`.

## List / show integration

- `deadreckon list` adds a `MATERIALIZED` column. Compute by checking for
  the `library/<scope>/<id>/.materialized-to` file. Show `no` /
  `yes (N times)` / `n/a` (for non-completed runs).
- `deadreckon show <run-id>` reveals lineage when `working/.deadreckon/parent.json`
  exists: prints "Extended from <parent-id>" before the run header.

## Post-run hints (`run`, `attach` on completed)

After a successful run completes (or when `attach` lands on a completed
run), print these two lines to stdout:

```
materialize: deadreckon materialize <id> --dest ./<task-prefix>
extend:      deadreckon extend <id> '<your follow-up goal>'
```

`<task-prefix>` is the first 24 chars of the task_key, sanitized to
filesystem-safe characters. The hints suppress with `--no-hints` (also
add this flag to `run` and `attach`).

## README / DESIGN updates

`/Users/gdc/deadreckon/README.md` gains a `## Lifecycle` section
documenting:

```
init → run → list → attach → materialize → (extend)
                              ↓
                              users' working dir (./my-project/)
```

With a 3-block example: starting a run, materializing on success, extending
later.

`/Users/gdc/deadreckon/DESIGN.md` CLI section lists the new verbs.

## Tests (must all pass)

New file: `/Users/gdc/deadreckon/crates/deadreckon/tests/lifecycle.rs`.

Required test names:

- `materialize_copies_library_to_dest`
- `materialize_refuses_existing_nonempty_dest`
- `materialize_force_overwrites`
- `materialize_writes_parent_manifest`
- `materialize_records_reverse_marker_in_library`
- `materialize_refuses_dest_inside_runstate`
- `extend_creates_new_run_with_parent_artifacts`
- `extend_pre_populates_history_with_parent_summary`
- `extend_refuses_incomplete_parent`
- `extend_locks_correctly_against_concurrent_extension`
- `extend_no_context_flag_omits_recent_turns`
- `materialize_then_extend_roundtrip`
- `list_shows_materialized_status`
- `show_reveals_parent_lineage`

Each uses the mock provider from `agentic_loop.rs`. Aim for ≤ 30 s per
test; the roundtrip test may go to 60 s.

## Engineering invariants additions (do not violate)

- **No new `PipelineState` fields.** If you find a reason to add one,
  surface it before proceeding — usually the right answer is a file in the
  working tree.
- **`copy_tree` is the only directory-copy primitive.** Reuse it; do not
  reinvent.
- **Locks remain task-scoped.** `extend` acquires the parent's task lock,
  not a new one.
- **Parent integrity.** `extend` never modifies the parent's
  `library/<scope>/<id>/` directory beyond appending to the
  `.materialized-to` marker (and only if the user runs `materialize`).
- **No new top-level deps** expected. If serializing the new JSON requires
  one, prefer adding to `serde_json` what already exists.

## Dependencies (per existing Tier 1 / 2 / 3 policy)

Tier 1 (utility): none expected; the work fits inside `std::fs` +
`serde_json` + existing helpers.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with the relevant subset of `lifecycle.rs` tests passing
  and a CHANGELOG entry naming the SHA.
- After completion, append a "Lifecycle ergonomics" section to
  `CHANGELOG.md` summarizing the two new verbs, the schema, and the
  hint-suppression flag.
