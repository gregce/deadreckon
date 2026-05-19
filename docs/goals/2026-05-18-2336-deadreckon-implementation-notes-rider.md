# deadreckon - Implementation Notes Rider (spec interpretation while building)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-18-2336-deadreckon-implementation-notes-goal.md`.
It supersedes nothing in prior riders
(`2026-05-11-1525-deadreckon-self-documenting-rider.md`,
`2026-05-11-2122-deadreckon-doc-depth-rider.md`,
`2026-05-17-1403-deadreckon-coherence-closure-rider.md`) - their invariants
still apply. This rider adds a live implementation-notes contract and evolves
`RUN-DECISIONS.md` into the Markdown ledger for implementation interpretation:
design decisions, deviations, tradeoffs, open questions, and separately
evidence-filtered multi-alternative decision details.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided - do not redesign)

- **Maturity stays `alpha`.** This is a behavior and documentation contract on
  top of existing run, doc, acceptance, and promotion machinery.
- **No `PipelineState` schema changes.** Notes freshness is derived from
  existing files and `_incremental.jsonl`; do not add state fields.
- **Converge on `RUN-DECISIONS.md`.** Do not rename it. Evolve it from a
  decision-only retrospective into the canonical implementation decision
  ledger, with the four interpretation sections plus a separate
  evidence-filtered multi-alternative decision details section.
- **The live artifact is `implementation-notes.html` at the run working root.**
  The exact filename is part of the contract because the user asked for it. It
  is the executor-maintained working copy; `RUN-DECISIONS.md` is the rendered
  Markdown copy exposed by `deadreckon doc --kind decisions`.
- **No new top-level verb.** Enrich the existing `decisions` doc kind and help
  text; keep the surface under existing `run`, `resume`, `doc`, `show`, and
  `apply` flows.
- **No provider-owned transcript rewrites.** Use prompts, seeded files,
  turn records, and checks; do not edit provider logs.
- **No `git push`.** Phased local commits only.
- **No silent V1 invention.** Major design expansions go to
  `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Data Model (files, not fields)

No durable schema fields are added.

Canonical live working-copy path:

```text
<working-dir>/implementation-notes.html
```

The file is intentionally in the working tree, not hidden under
`.deadreckon/docs`, because the executor must maintain it while implementing and
the owner should be able to inspect it before promotion. The promoted library
therefore contains the same file as part of the artifact tree.

Minimum HTML shape:

```html
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>Implementation notes</title>
</head>
<body>
  <h1>Implementation notes</h1>
  <dl>
    <dt>Run</dt><dd>RUN_ID</dd>
    <dt>Goal</dt><dd>GOAL</dd>
    <dt>Last updated</dt><dd>RFC3339_TIME</dd>
  </dl>
  <section id="design-decisions"><h2>Design decisions</h2><p>None yet.</p></section>
  <section id="deviations"><h2>Deviations</h2><p>None.</p></section>
  <section id="tradeoffs"><h2>Tradeoffs</h2><p>None yet.</p></section>
  <section id="open-questions"><h2>Open questions</h2><p>None.</p></section>
</body>
</html>
```

The executor may replace placeholder paragraphs with lists, tables, or concise
prose. The required section ids and headings are a depth-tested contract.

Canonical Markdown ledger path:

```text
<working-dir>/.deadreckon/docs/RUN-DECISIONS.md
```

Promoted/public copy:

```text
<library-artifact>/docs/RUN-DECISIONS.md
```

Minimum `RUN-DECISIONS.md` shape:

```markdown
# <goal-derived title>

This document captures implementation decisions and spec interpretation for run `<short-id>`.
Live working copy: [`implementation-notes.html`](../../implementation-notes.html)

## Design decisions

<content from implementation-notes.html or "None.">

## Deviations

<content from implementation-notes.html or "None.">

## Tradeoffs

<content from implementation-notes.html or "None.">

## Open questions

<content from implementation-notes.html or "None.">

## Multi-alternative decision details

No multi-alternative decisions detected in this run.

## Turn citations
```

The old canonical no-decisions sentence moves under
`## Multi-alternative decision details`; it must not replace the four
implementation-interpretation sections.

When publishing to public `docs/RUN-DECISIONS.md`, rewrite the live-working-copy
link to `../implementation-notes.html` if the file is present in the promoted
artifact root.

## SPEC Resolution

For prompts and refusal messages, `SPEC` means:

1. The user goal string stored in `PipelineState.goal`.
2. `acceptance.md` when present in the run root.
3. `acceptance.yaml` when present in the run root, including the project or
   `--spec` acceptance file copied at run start.
4. Any worker spec injected by plan/orchestration flows.

Do not introduce a separate `--spec` flag for implementation notes. The CLI
already has acceptance spec plumbing; this rider changes how the run loop briefs
the executor and validates completion.

## Prompt Contract

Both provider paths receive the same semantic contract:

```text
Implement the SPEC in the working directory.

As you work, maintain implementation-notes.html at the working-directory root.
Keep it current with anything the owner should know about how the implementation
interprets or diverges from the spec:
- Design decisions: choices made where the spec was ambiguous.
- Deviations: intentional departures from the spec, with reasons.
- Tradeoffs: alternatives considered and why the chosen path won.
- Open questions: anything the owner should confirm or revise.

Before reporting done, update implementation-notes.html after the latest
documentable code/config/test/doc change. If there is nothing to report in a
section, say "None" rather than deleting the section.
```

The run docs will render the same content into `RUN-DECISIONS.md`; the prompt
should describe this so the executor understands that the HTML file is not a
throwaway sidecar. The executor should not edit `.deadreckon/docs/RUN-DECISIONS.md`
directly unless it is implementing deadreckon's own docs machinery.

The action JSON requirement for non-CLI providers must remain the final
instruction in `build_prompt`; do not let skill text obscure the schema.

## Notes And Decisions Freshness Algorithm

Use existing turn records; do not add state fields.

Definitions:

- `notes_path = working_dir / "implementation-notes.html"`.
- `notes_turn = latest turn whose changed files include notes_path`.
- `implementation_turn = latest turn whose changed files include a documentable
  source, config, manifest, test, asset, or project-doc file other than
  `implementation-notes.html`, generated files, run artifacts, snapshots, and
  provider logs.

Completion is allowed when:

1. `implementation-notes.html` exists.
2. The HTML contains the four required section ids and headings.
3. `notes_turn >= implementation_turn`, or there has been no implementation
   turn yet.

After the check passes, the deterministic doc rewrite must render the current
notes content into `RUN-DECISIONS.md` before acceptance/promotion. If provider
polish later runs, it may improve prose but must preserve the four sections and
the multi-alternative details section.

When the check fails on an `Action::Done` turn or after a CLI subagent returns,
do not fail the run. Append a concise history message telling the provider to
update `implementation-notes.html`, emit a normal run event if the local event
vocabulary has a suitable warning/status kind, save state, and continue the
loop. If the max-turn budget is exhausted, the existing failure path applies.

## Relationship To Existing Docs

- `RUN-NARRATIVE.md` remains chronological.
- `RUN-AS-BUILT.md` remains subsystem-oriented.
- `RUN-DECISIONS.md` becomes the canonical Markdown implementation decision
  ledger: four interpretation sections plus evidence-filtered decision details.
- `implementation-notes.html` is the live, owner-facing working copy that feeds
  the decisions ledger.

`narrator-decisions` should output the four implementation interpretation
sections from notes/trace evidence and separately output real multi-alternative
decision details. If no multi-alternative decision appears in trace evidence,
only the details section gets the canonical "No multi-alternative decisions
detected in this run." line.

`deadreckon doc <run-id> --kind decisions` is the primary inspection command
for the converged ledger. `deadreckon doc <run-id> --kind implementation-notes`
may read the promoted or working `implementation-notes.html` directly as a
convenience, but do not make the HTML doc kind the only path to the new content.
Do not run provider polish over the HTML file; the executor owns it.

## Phases (eleven)

Each phase: write the named depth test(s) first and watch them fail; implement;
run focused verification for the touched surface; conventional local commit;
one-line CHANGELOG entry when user-visible behavior changes.

### P1 - Freeze the current gap

- Add tests proving the current prompt does not yet contain the spec-first
  implementation-notes contract and current `RUN-DECISIONS.md` does not expose
  the four implementation-interpretation sections.
- Keep these tests narrow enough to fail for the missing behavior, not for
  incidental prompt wording.

Depth tests:
- `run_prompt_names_implement_spec_and_implementation_notes_contract`
- `run_decisions_includes_implementation_interpretation_sections`
- `done_without_current_implementation_notes_is_rejected`

### P2 - Notes path and HTML template helpers

- Add constants/helpers in `deadreckon-core` for
  `implementation-notes.html`.
- Add `ensure_implementation_notes_started(state)` that creates the file only
  when absent and preserves user/provider edits when present.
- Add a small extractor/renderer that maps the HTML sections to
  `RUN-DECISIONS.md` sections.
- Keep the template ASCII, valid HTML, and deterministic apart from run id,
  goal, and timestamp.

Depth tests:
- `implementation_notes_seed_writes_required_html_sections`
- `implementation_notes_seed_preserves_existing_file`
- `implementation_notes_path_is_working_root_file`
- `implementation_notes_sections_render_into_run_decisions`

### P3 - Prompt frame and skill contract

- Update `skills/default-coding/SKILL.md` with the notes contract.
- Make the runtime prompt include the selected skill content or a small fallback
  contract when the skill file cannot be read. Preserve the final action JSON
  instruction for non-CLI providers.
- Update CLI-subagent prompt with the same contract using direct-edit language.
- The prompt should name `RUN-DECISIONS.md` as the published Markdown ledger so
  the executor sees one converged documentation story.

Depth tests:
- `default_coding_skill_mentions_all_four_notes_sections`
- `json_provider_prompt_keeps_action_schema_last`
- `cli_subagent_prompt_includes_same_notes_contract`
- `prompt_names_run_decisions_as_published_ledger`

### P4 - Freshness detector

- Implement a pure helper that reads `_incremental.jsonl` and decides whether
  notes are present, structurally valid, and current relative to documentable
  implementation turns.
- Implement deterministic projection from valid notes into `RUN-DECISIONS.md`.
- Reuse the existing documentable-path filter where possible.
- Exclude the notes file itself from `implementation_turn`.

Depth tests:
- `notes_freshness_passes_when_notes_turn_follows_code_turn`
- `notes_freshness_fails_when_code_turn_follows_notes_turn`
- `notes_freshness_ignores_generated_run_artifact_changes`
- `notes_freshness_requires_four_sections`
- `run_decisions_projection_preserves_no_multi_alternative_message`

### P5 - Run-loop done refusal

- Gate `Action::Done` through the freshness helper before acceptance polish and
  promotion.
- On stale/missing notes, append an actionable history line and continue the run
  instead of failing immediately.
- When notes are current, rewrite `RUN-DECISIONS.md` from the notes content
  before acceptance/promotion.
- Keep existing acceptance-gate behavior unchanged after notes pass.

Depth tests:
- `done_action_continues_when_implementation_notes_missing`
- `done_action_history_tells_provider_to_update_notes`
- `done_action_runs_acceptance_only_after_notes_are_current`
- `done_action_updates_run_decisions_from_current_notes`

### P6 - CLI-subagent parity

- Apply the same freshness gate after CLI subagent file changes and before
  docs/acceptance/promotion.
- Do not turn stale notes into the existing "completed without file changes"
  failure; request a follow-up turn when files changed but notes are stale.

Depth tests:
- `cli_subagent_completion_requires_current_implementation_notes`
- `cli_subagent_followup_turn_can_update_notes_and_complete`
- `cli_subagent_without_file_changes_still_uses_existing_failure`

### P7 - Decisions doc and export surface

- Make `deadreckon doc <run-id> --kind decisions` print the converged ledger
  with all four implementation-interpretation sections.
- Keep `--export` and `--overwrite` behavior for the decisions doc unchanged.
- Optionally add `ImplementationNotes` to `DocKind` and `CliDocKind` as a
  direct HTML convenience, but it is not a substitute for enriching decisions.
- Error footer when absent:
  `try: deadreckon resume <run-id>` for live runs or
  `try: inspect <library-path>/implementation-notes.html` when the run is
  already promoted but the artifact is missing.

Depth tests:
- `doc_kind_decisions_prints_implementation_interpretation_sections`
- `doc_kind_decisions_exports_converged_ledger`
- `doc_kind_decisions_missing_notes_has_try_line`

### P8 - Narrator decisions convergence

- Update `narrator-decisions` prompt and renderer so split polish can return
  four interpretation sections plus a separate `decisions` array.
- Include a short cross-reference from `RUN-DECISIONS.md` to
  `implementation-notes.html` when the file exists.
- Keep false-positive filtering and put the canonical no-decisions line only in
  `## Multi-alternative decision details`.

Depth tests:
- `narrator_decisions_prompt_outputs_interpretation_sections`
- `templated_decisions_cross_links_implementation_notes_when_present`
- `no_multi_alternative_message_survives_inside_details_section`

### P9 - Promotion, apply, and worktree behavior

- Ensure normal promotion preserves the root notes file in the library artifact.
- Ensure promoted `docs/RUN-DECISIONS.md` includes the converged sections and
  links back to the root `implementation-notes.html`.
- Ensure worktree apply treats the notes file like a normal changed project doc.
- Ensure `changed_doc_files` and diff coverage do not hide source changes
  behind notes-only edits.

Depth tests:
- `promoted_library_contains_implementation_notes_html`
- `promoted_run_decisions_contains_converged_sections`
- `worktree_apply_includes_implementation_notes_when_changed`
- `diff_coverage_still_requires_source_files_with_notes_present`

### P10 - Friendly output and help

- Add help/example text to `doc`, `run` preflight/start summaries, and any run
  completion summary that already names docs.
- Keep status/list surfaces quiet; only add a concise notes path or stale-notes
  hint where the run lifecycle already prints next actions.
- Prefer hints that point to `deadreckon doc <run-id> --kind decisions`; mention
  the HTML file as the live working copy path.
- Use existing `try:` and lifecycle hint helpers.

Depth tests:
- `run_start_summary_mentions_implementation_notes_path`
- `run_completion_summary_points_to_decisions_doc_kind`
- `stale_notes_refusal_uses_try_line`

### P11 - Docs and final audit

- Update `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` in section 25 and any
  relevant CLI surface section.
- Update `/Users/gdc/deadreckon/CHANGELOG.md` with an
  "Implementation notes (alpha)" entry.
- If this closes or reframes a thin item, update section 22; otherwise say no thin item
  was closed.
- Add any newly deferred larger idea to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

Depth tests:
- No new code depth test; docs-only phase. Run the final focused verification
  set from the goal.

## Error-Footer Canonical Pairs

| Error | `try:` |
|---|---|
| `implementation-notes.html is missing` | `deadreckon resume <run-id>` |
| `implementation-notes.html is stale` | `edit implementation-notes.html, then deadreckon resume <run-id>` |
| `implementation-notes.html is missing required sections` | `add Design decisions, Deviations, Tradeoffs, and Open questions sections` |
| `RUN-DECISIONS.md is missing implementation sections` | `deadreckon doc <run-id> --polish --kind decisions --no-confirm` |
| `decisions doc is unavailable for this run` | `deadreckon doc <run-id> --kind narrative` |

Every error footer above must be exercised by a focused test.

## Config additions

No config keys in this milestone.

Do not add a `defaults.implementation_notes` toggle unless the implementation
shows a real compatibility problem. The requested behavior is the new default
for implementation runs.

## Out of scope

- Markdown notes instead of HTML.
- A rich HTML editor, CSS theme, screenshots, or browser preview.
- Provider polish that rewrites `implementation-notes.html`.
- Removing `RUN-DECISIONS.md` or renaming it away from existing doc-kind APIs.
- A separate `implement` top-level verb.
- Persistent DB/state fields for notes freshness.
- Multi-file notes split by phase or component.
- Enforcing semantic completeness of note prose beyond structural sections and
  freshness relative to changed files.

## Dependencies

Tier 1 (utility, free): none expected. Use Rust standard library string checks
for the required HTML sections unless existing HTML parsing utilities already
exist locally.

Tier 2 (architectural): none expected.

Tier 3 (blocked): new browser/HTML parser, database, or formatting engine.

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes.**
- **`RUN-DECISIONS.md` is the convergence point.** It must include the four
  implementation-interpretation sections, while multi-alternative decision
  details remain evidence-filtered.
- **The exact file name `implementation-notes.html` is stable.**
- **Notes must be current before acceptance/promotion.** A successful run with
  documentable implementation changes and stale notes is a bug.
- **Prompt schema stays enforceable.** Non-CLI providers must still return one
  JSON action object.
- **No silent expansion.** Anything beyond P1-P11 goes into
  `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase starts with named depth tests and ends with the focused tests for
  that surface green.
- Use focused verification by default:
  `cargo test -p deadreckon self_documenting`,
  `cargo test -p deadreckon agentic_loop`,
  `cargo test -p deadreckon doc_kind`,
  `cargo test -p deadreckon-runtime implementation_notes`,
  `cargo fmt --check`, and targeted
  `cargo clippy -p deadreckon --all-targets -- -D warnings` after code changes.
- Run broader workspace verification only if the implementation touches broader
  crate contracts or the user explicitly asks for it.
