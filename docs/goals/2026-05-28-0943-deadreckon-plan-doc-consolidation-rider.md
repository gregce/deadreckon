# deadreckon - Plan Doc Consolidation Rider (Consolidated)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-28-0943-deadreckon-plan-doc-consolidation-goal.md`.
It supersedes nothing in prior riders, especially self-documenting runs, doc
depth, orchestration event bus, narrative attach, merge repair, guided start,
and production command model. Their invariants still apply. This rider adds a
consolidated documentation projection for orchestration plans and their merged
or applyable results.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime
`/Users/gdc/.deadreckon`.

## Posture (decided - do not redesign)

- **Maturity target is production release.** This work should leave plan-result
  documentation reliable enough for real users and release notes, not framed as
  an alpha-only experiment.
- **Files, not fields.** Do not add `PipelineState`, `Plan`, `PlanTask`,
  provider registry, or manifest schema fields until a files-not-fields option
  below has been tried and fails a depth test.
- **Plan docs are projections.** Source of truth remains plan JSON,
  plan-events JSONL, child run state, child docs, task summaries, traces,
  acceptance, and merge manifests.
- **Provider-backed, fallback-first.** A configured doc provider improves the
  prose. Missing or failing provider calls must not block merge, apply, export,
  finish, or library promotion.
- **Do not mutate child runs.** Consolidation reads child artifacts and may
  generate plan-level docs; it does not rewrite a child run's docs, traces, or
  state.
- **No forced child reruns.** If child docs are missing or thin because the
  orchestration runner launched children with `--no-docs`, use summaries and
  plan evidence, and record the gap. A later phase may add an opt-in plan-doc
  polish pass, but it must not surprise-run providers.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Remote publishing, cloud plan notebooks, personal doc
  style learning, arbitrary repo mining, and collaborative editing go to
  `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.
- **Edits stay inside `/Users/gdc/deadreckon`.**

## What Production Release Means Here

In older riders, "alpha" meant provisional behavior: acceptable rough edges,
thin fallbacks, and honest limits while the shape was still being discovered.
For this goal, the posture changes. Plan-result docs become part of the product
contract:

- no completed orchestration result should present empty wrapper docs as the
  main story;
- provider failures degrade to complete deterministic docs, not missing docs;
- user-facing commands must point to the right plan docs without requiring
  source-code knowledge;
- tests must cover the plan id, merged-result run id, and synthetic apply-run
  id paths;
- documentation and CHANGELOG wording should describe shipped behavior, not
  caveats around a prototype.

## Current Diagnosis

The observed failure mode is real and local to plan-result documentation, not
child execution:

- Plan `e6db8e0d...` has `/Users/gdc/.deadreckon/plans/<plan-id>/docs/PLAN-NARRATIVE.md`.
- The plan dir also has `summaries/task-*.md`, `worker-specs/task-*.md`,
  `plan.json`, `messages.jsonl`, and `plan-events.jsonl`.
- Plan-result apply run `4ca07d2a...` is a synthetic wrapper. Its trace only
  records `plan_result_apply_prepared`; it has no provider turns.
- `prepare_plan_result_apply_state` in
  `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs` creates that wrapper
  with provider `deadreckon:orchestrate-apply`.
- `create_run` calls `ensure_docs_started`, which writes templated
  `.deadreckon/docs/RUN-NARRATIVE.md`, `RUN-AS-BUILT.md`, and
  `RUN-DECISIONS.md` for the wrapper.
- `skip_plan_merge_file` skips `.deadreckon`, `implementation-notes.html`, and
  public `docs/RUN-*`.
- `skip_plan_apply_file` additionally skips `deadreckon-plan-manifest.json`.
- `seed_plan_result_worktree` copies the merged source to the apply worktree
  through those skip rules, so plan docs do not naturally materialize there.
- `launch_plan_child` currently passes `--no-docs`; child docs may therefore be
  templated or unpolished even when summaries exist.

The fix is not "make the synthetic run pretend it did child work." The fix is a
plan-level documentation bundle and surfaces that know when a run is a
plan-result wrapper.

## Data Model (files, not fields)

### `<plan-dir>/docs/PLAN-DOCS-MANIFEST.json`

Atomically rewritten after each successful consolidation attempt.

```json
{
  "schema_version": 1,
  "plan_id": "e6db8e0dd0bd4d9d9d90b2b1052310cf",
  "root_goal": "build ...",
  "created_at": "2026-05-28T13:43:00Z",
  "updated_at": "2026-05-28T13:43:00Z",
  "status": "provider|deterministic|failed_provider_fallback",
  "input_hash": "sha256:...",
  "provider": {
    "route": "cli:codex",
    "source": "flag|config|plan|child|none",
    "calls": 1,
    "cost_usd": 0.0,
    "duration_ms": 12345
  },
  "children": [
    {
      "task_id": "task-0",
      "task_index": 0,
      "depends_on": [],
      "child_run_id": "064466269e184b22947e427a36c79551",
      "status": "completed",
      "provider": "cli:claude-code",
      "doc_sources": [
        ".deadreckon/docs/RUN-NARRATIVE.md",
        ".deadreckon/docs/RUN-AS-BUILT.md",
        ".deadreckon/docs/RUN-DECISIONS.md",
        "summary"
      ],
      "doc_status": "polished|templated|missing|summary_only"
    }
  ],
  "outputs": [
    "docs/PLAN-NARRATIVE.md",
    "docs/PLAN-AS-BUILT.md",
    "docs/PLAN-DECISIONS.md",
    "docs/PLAN-CHILDREN.md"
  ],
  "warnings": []
}
```

### `<plan-dir>/docs/_plan-docs.jsonl`

Append-only consolidation log. Each row is one event:

```json
{
  "schema_version": 1,
  "timestamp": "2026-05-28T13:43:00Z",
  "event": "collected|provider_requested|provider_completed|provider_failed|validated|written",
  "plan_id": "e6db8e0d...",
  "input_hash": "sha256:...",
  "detail": {}
}
```

### `<plan-dir>/docs/plan-doc-input.json`

Bounded provider input bundle with redacted snippets and evidence ids. This is
debuggable, deterministic, and safe to inspect. It may omit long raw text once a
content hash and excerpt are recorded.

### `<plan-dir>/docs/plan-doc-provider-response.json`

Structured provider output before Markdown rendering. If the provider fails,
write `plan-doc-provider-error.json` with refusal/error metadata and fallback
reason. Do not store unbounded raw terminal output.

### Consolidated Markdown outputs

Write these under `<plan-dir>/docs/`:

- `PLAN-NARRATIVE.md` - what happened across the plan, reading order, task
  graph, current/final outcome, repairs, acceptance, open threads.
- `PLAN-AS-BUILT.md` - merged system architecture and changed artifact map,
  synthesized from child docs plus the merge result.
- `PLAN-DECISIONS.md` - cross-child decisions, conflicts, merge repairs,
  tradeoffs, assumptions, and deferrals.
- `PLAN-CHILDREN.md` - task-by-task index with child run ids, provider, status,
  doc status, summaries, and links.

Mirror the same files into merged result libraries and plan apply/export
worktrees:

- `.deadreckon/docs/PLAN-NARRATIVE.md`
- `.deadreckon/docs/PLAN-AS-BUILT.md`
- `.deadreckon/docs/PLAN-DECISIONS.md`
- `.deadreckon/docs/PLAN-CHILDREN.md`
- `docs/PLAN-NARRATIVE.md`
- `docs/PLAN-AS-BUILT.md`
- `docs/PLAN-DECISIONS.md`
- `docs/PLAN-CHILDREN.md`

For synthetic plan-result wrapper runs, keep `RUN-*` files but rewrite them as
thin wrappers:

- `RUN-NARRATIVE.md` says this run materializes plan `<plan-id>` result
  `<merged-run-id>` and links to `PLAN-NARRATIVE.md`.
- `RUN-AS-BUILT.md` links to `PLAN-AS-BUILT.md`.
- `RUN-DECISIONS.md` links to `PLAN-DECISIONS.md`.

Do not let a zero-turn wrapper overwrite or hide the plan-level docs.

## Input Collection Algorithm

Implement a collector that accepts a plan id, merged run id, or plan-result
apply run id.

Resolution rules:

1. If the id resolves to a plan, use it directly.
2. If the id resolves to a run with trace event `plan_result_apply_prepared`,
   read `detail.plan_id` and `detail.merged_run_id`.
3. If the id resolves to a merged plan run, use traces/manifests to recover the
   plan id where available. If not available, refuse with a `try:` line that
   names `deadreckon show <id> --json`.
4. If the id is `latest`, keep existing latest semantics but prefer a completed
   plan/apply result in the current scope when the command is plan-doc specific.

Task ordering:

- Topologically sort by `depends_on`.
- Use numeric `PlanTask.index` or lexical `task_id` as a deterministic tie
  breaker for independent siblings.
- Include repair runs and merge repair summaries after the children whose
  outputs they reconciled.
- Emit the chosen order in `PLAN-CHILDREN.md` and the manifest.

Per child, collect in this order:

1. `PlanTask` metadata: id, index, role, subject, goal, provider, status,
   dependencies, child run id.
2. Worker spec: `<plan-dir>/worker-specs/<task-id>.md`.
3. Task summary: `<plan-dir>/summaries/<task-id>.md`.
4. Child run state from `load_run`.
5. Child promoted artifact root: prefer `paths.library_dir(scope, run_id)`,
   fall back to `state.working_dir` when the library is missing.
6. Child internal docs:
   - `.deadreckon/docs/RUN-NARRATIVE.md`
   - `.deadreckon/docs/RUN-AS-BUILT.md`
   - `.deadreckon/docs/RUN-DECISIONS.md`
7. Child public docs:
   - `docs/RUN-NARRATIVE.md`
   - `docs/RUN-AS-BUILT.md`
   - `docs/RUN-DECISIONS.md`
8. Child traces/events only as bounded fallback evidence; do not feed whole
   logs to the provider.
9. Acceptance result and file inventory.

Doc status rules:

- `polished`: doc exists, is non-empty, and does not contain only templated
  zero-turn text.
- `templated`: doc exists but says `Doc-writer: templated only` or "No
  completed turns have been recorded yet."
- `summary_only`: docs are missing or templated, but `summaries/<task-id>.md`
  exists.
- `missing`: no useful docs and no summary. The output must name this gap.

Input bounds:

- Cap each child narrative excerpt at 30 KB.
- Cap each child as-built excerpt at 40 KB.
- Cap each child decisions excerpt at 20 KB.
- Cap worker spec and summary at 15 KB each.
- Preserve full file hashes and byte lengths for omitted content.
- Redact obvious secrets before writing provider input files.

## Provider Consolidation

Use the existing documentation provider routing model:

- Explicit command flag wins.
- Then configured doc provider.
- Then a plan-level repair/doc provider if one already exists in plan metadata.
- Then deterministic fallback.

Do not add a new provider registry or config schema. If the current code lacks a
clean helper for "resolve a doc provider for this plan," add a local helper that
returns the same `DocProviderSelection` shape used by run docs.

Provider request requirements:

- Ask for structured JSON, not direct Markdown.
- Include the task graph and child evidence ids.
- Tell the model to consolidate, not concatenate.
- Require citations on each substantive claim.
- Require an "unknowns / missing evidence" section.
- Require child coverage: every completed child appears in at least one
  narrative/as-built/decision item or in a "no relevant doc contribution" list.

Provider response shape:

```json
{
  "schema_version": 1,
  "title": "Plan result for ...",
  "narrative": {
    "summary": "...",
    "task_graph": [],
    "phases": [],
    "repairs": [],
    "acceptance": [],
    "open_threads": []
  },
  "as_built": {
    "system_overview": "...",
    "components": [],
    "changed_files": [],
    "runtime_notes": []
  },
  "decisions": {
    "decisions": [],
    "tradeoffs": [],
    "deferrals": []
  },
  "children": []
}
```

Validation:

- Reject unknown citation ids.
- Reject file paths not found in child inventories, merge-working, or the final
  result inventory unless the item is explicitly marked as "planned but absent."
- Reject outputs that omit a completed child.
- Reject outputs containing raw secret-like strings after redaction.
- Reject docs below a minimum useful size unless the plan is genuinely tiny.

On validation failure, write provider error metadata and produce deterministic
fallback docs. Do not fail the plan merge/apply because the prose provider did
not cooperate.

## Deterministic Fallback

Fallback docs are first-class, not a last-ditch error page.

`PLAN-NARRATIVE.md` fallback must include:

- Title, plan id, root goal, status, generated time, provider/fallback status.
- Reading order.
- Task graph in topological order.
- One section per child: goal, provider, status, child run id, summary excerpt,
  doc status, changed-file count, acceptance/gate notes.
- Merge/repair section when proof files or repair runs exist.
- Final result section naming merged run id and apply/export worktree when known.
- Missing evidence section.

`PLAN-AS-BUILT.md` fallback must include:

- Consolidated file inventory from merge-working or merged result.
- Component grouping using existing docs path classification helpers when
  possible.
- Per-child contributions and touched paths.
- Runtime/build/test/deploy notes from summaries and manifests.

`PLAN-DECISIONS.md` fallback must include:

- Explicit decisions detected in child `RUN-DECISIONS.md`.
- Merge decisions and repair choices.
- Assumptions and deferrals from summaries.
- Honest "no explicit decisions found" rows per child when appropriate.

`PLAN-CHILDREN.md` fallback must always be useful and complete.

## CLI and Surface Contract

Do not add a new top-level command unless existing `doc`/`docs` cannot be
extended cleanly.

Preferred shape:

```text
deadreckon doc <plan-id-or-result-run-id> --plan
deadreckon docs <plan-id-or-result-run-id> --plan
    --kind narrative|as-built|decisions|children|all
    --refresh
    --doc-provider <route>
    --no-provider
    --export <path>
    --json
```

Acceptable if it fits clap better:

```text
deadreckon doc plan <plan-id-or-result-run-id>
deadreckon docs plan <plan-id-or-result-run-id>
```

Surface rules:

- `deadreckon merge <plan-id>` runs consolidation after a successful merge and
  before printing finish/apply/export hints.
- `deadreckon finish <plan-id>` and `deadreckon apply <plan-id>` ensure plan
  docs are materialized before preparing the result worktree.
- `deadreckon export <plan-id>` includes plan docs in the exported artifact.
- `deadreckon show <plan-id>` lists plan docs and doc status.
- `deadreckon show <plan-result-apply-run>` says it is a synthetic wrapper and
  points to plan docs.
- `deadreckon attach <plan-id> --view narrative` may keep using live narrative
  snapshots while running, but completed plans should offer consolidated
  `PLAN-NARRATIVE.md` via the docs toggle.
- `deadreckon attach <plan-result-apply-run>` should not show empty
  zero-turn docs as the primary story.

Refusals:

| Case | Required behavior |
|---|---|
| Unknown id | Refuse with `try: deadreckon list` and `try: deadreckon show <id>` |
| Plan not terminal | Show current status; allow deterministic preview only if safe |
| Child still running | Refuse refresh by default; suggest `deadreckon attach <plan-id>` |
| No child docs/summaries | Write missing-evidence docs, warn, and keep going |
| Provider unavailable | Deterministic fallback, not command failure |
| Provider validation failed | Record error, deterministic fallback, warning |
| Export path exists | Reuse existing export overwrite/force convention |

## Merge, Apply, and Copy Rules

Do not simply remove the `.deadreckon` skip rule globally; that could copy
internal logs into user artifacts. Instead:

- Keep broad internal skip rules for child artifacts.
- Add explicit plan-doc materialization after merge/apply/export copy steps.
- Copy only the allowlisted plan doc files and their manifest/log metadata.
- Public `docs/PLAN-*` should be copied; public `docs/RUN-*` for child runs
  should still be skipped unless the rider phase explicitly changes that.
- Synthetic wrapper `RUN-*` docs may be generated in the apply worktree because
  they describe the wrapper, not a child run.

The implementation should have one helper responsible for writing/copying plan
docs into a destination working tree so merge, apply, export, and tests share
the same behavior.

## Privacy and Redaction

Before provider calls:

- Reuse existing redaction helpers if present.
- Detect obvious token/key/password/private-key/connection-string shapes.
- Treat `.env`, credentials files, provider logs, and raw shell output as high
  risk.
- Prefer summaries and docs over raw logs.
- Record redaction counts in the manifest.

Provider prompts must say that missing evidence should be named as missing, not
filled in creatively.

## Phases (eleven)

Each phase: write the named depth test(s) first and watch them fail; implement;
run focused verification green; make a conventional local commit; add a
one-line CHANGELOG entry. If a phase is too large, split it into smaller local
commits without weakening the tests.

### P1 - Reproduce the Synthetic Wrapper Gap

- Build fixtures for a completed full-plan with at least two children, a merged
  result run, and a plan-result apply wrapper.
- Include child docs that are polished, templated, summary-only, and missing.
- Capture the current `4ca07d2a` class: wrapper docs exist but contain no plan
  story.

Depth tests:

- `plan_result_apply_docs_do_not_replace_plan_rollup_with_empty_run_docs`
- `plan_docs_fixture_marks_templated_child_docs_as_templated`
- `plan_docs_fixture_uses_task_summary_when_child_docs_are_thin`

### P2 - Plan Doc Input Collector

- Add a collector that resolves plan ids, merged run ids, and synthetic apply
  run ids.
- Topologically order tasks and include deterministic tie-breaking.
- Read worker specs, summaries, child state, child docs, merge proof metadata,
  acceptance status, and final result inventory.
- Write `plan-doc-input.json` and manifest child entries.

Depth tests:

- `plan_docs_collect_child_run_docs_in_task_graph_order`
- `plan_docs_resolve_apply_wrapper_back_to_plan_and_merged_run`
- `plan_docs_collect_repair_runs_after_conflicting_children`
- `plan_docs_inventory_records_missing_and_templated_sources`

### P3 - Deterministic Plan Docs

- Render all four plan docs without a provider.
- Make fallback docs readable enough to be useful in CI and offline use.
- Include stable citations or evidence labels for child run ids, task ids,
  summaries, and files.

Depth tests:

- `plan_docs_fallback_writes_narrative_as_built_decisions_and_children`
- `plan_docs_fallback_names_every_completed_child`
- `plan_docs_fallback_includes_missing_evidence_section`
- `plan_docs_fallback_uses_merge_result_inventory`

### P4 - Provider Request and Response Schema

- Define bounded request and response structs.
- Redact before request persistence and provider invocation.
- Validate citations, child coverage, file paths, minimum usefulness, and secret
  leakage.

Depth tests:

- `plan_docs_provider_request_uses_bounded_redacted_child_docs`
- `plan_docs_provider_response_rejects_unknown_citations`
- `plan_docs_provider_response_rejects_invented_paths`
- `plan_docs_provider_response_rejects_missing_completed_child`

### P5 - Provider-Backed Consolidation

- Invoke the existing doc provider route for plan consolidation.
- Store provider metadata in the manifest and `_plan-docs.jsonl`.
- Render validated provider JSON to Markdown.
- Fall back deterministically on provider errors or validation failure.

Depth tests:

- `plan_docs_provider_consolidates_child_docs_with_citations`
- `plan_docs_provider_failure_writes_deterministic_fallback`
- `plan_docs_provider_validation_failure_keeps_merge_successful`
- `plan_docs_no_provider_flag_skips_provider_call`

### P6 - CLI Plan Docs Refresh

- Extend `deadreckon doc`/`docs` to target plans and plan-result wrapper runs.
- Support refresh, provider selection, no-provider, JSON status, and export.
- Preserve existing run-doc behavior.

Depth tests:

- `doc_plan_refresh_writes_plan_docs_for_plan_id`
- `doc_plan_refresh_accepts_plan_result_apply_run_id`
- `doc_run_existing_behavior_is_unchanged`
- `doc_plan_json_reports_provider_and_child_doc_status`

### P7 - Merge and Finish Integration

- After successful `deadreckon merge <plan-id>`, consolidate docs and copy them
  into the merged result library.
- Ensure `finish <plan-id>` sees the plan-doc status and prints useful next
  actions.
- Keep provider failure nonfatal.

Depth tests:

- `plan_merge_writes_consolidated_docs_to_merged_library`
- `plan_merge_provider_failure_still_promotes_with_fallback_docs`
- `finish_plan_reports_consolidated_doc_paths`

### P8 - Apply and Export Materialization

- Materialize allowlisted plan docs into plan apply worktrees and exported
  artifacts.
- Rewrite synthetic wrapper `RUN-*` docs as cross-link wrappers.
- Keep broad `.deadreckon` and child `RUN-*` skip rules intact.

Depth tests:

- `plan_apply_worktree_contains_plan_docs_and_run_docs_crosslink`
- `plan_export_contains_public_plan_docs`
- `plan_apply_does_not_copy_child_internal_logs`
- `plan_apply_commit_body_mentions_plan_docs`

### P9 - Show, Attach, and Library Surfaces

- `show` should display plan doc status for plans and plan-result wrapper runs.
- Completed plan attach docs view should prefer consolidated plan docs.
- Library search/list should index or at least report `PLAN-*` docs for merged
  plan results.

Depth tests:

- `show_plan_result_prefers_plan_narrative_docs`
- `attach_completed_plan_docs_toggle_reads_consolidated_plan_docs`
- `library_plan_result_reports_plan_docs_inventory`

### P10 - Child Doc Quality and Opt-In Polish

- Decide whether plan children should continue launching with `--no-docs`.
- If changing that behavior is too expensive or risky, add an explicit
  `--plan-docs-polish-children` or equivalent refresh path rather than silently
  running providers.
- Record doc quality per child in `PLAN-CHILDREN.md`.

Depth tests:

- `plan_docs_marks_no_docs_child_as_summary_only`
- `plan_docs_child_polish_is_opt_in`
- `plan_docs_do_not_rerun_child_provider_without_flag`

### P11 - Architecture Docs and CHANGELOG

- Update `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` with a section
  for consolidated plan docs, including files, provider fallback, and surfaces.
- Update README/HOWTO only where first-contact guidance mentions plan result
  docs or `doc`/`docs`.
- Update `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` for deferred questions.
- Add a concise CHANGELOG section for "Plan Doc Consolidation (production
  release)".

No depth test, but run:

- `cargo fmt --check`
- focused tests touched by P1-P10
- `git diff --check`

## Out of Scope

- Web publishing or deployment of docs.
- Editing child run docs in place.
- Re-running child agents to improve docs.
- Replacing run-level `RUN-*` docs.
- A long-lived plan-doc daemon.
- Cloud storage, remote sharing, or collaboration.
- Arbitrary historical migration of all old plans. A manual refresh command is
  enough for old plan ids.
- Making provider-generated docs authoritative over source evidence.

## Stop Conditions

The goal is done when:

- A plan id, merged plan result run id, and synthetic plan-result apply run id
  all resolve to the same consolidated plan docs.
- Provider-backed consolidation produces validated, cited docs.
- Provider-disabled and provider-failed paths produce deterministic fallback
  docs.
- Merge/apply/export artifacts include `PLAN-*` docs and wrapper `RUN-*`
  cross-links.
- Empty zero-turn synthetic docs no longer mask the plan story.
- Focused tests, `cargo fmt --check`, and `git diff --check` pass.
- AS-BUILT and CHANGELOG document the production-release behavior.
- Work is committed locally.
