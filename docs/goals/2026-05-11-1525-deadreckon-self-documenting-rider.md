# deadreckon — Self-Documenting Runs Rider (stoa shape, auto-generated)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-11-1525-deadreckon-self-documenting-goal.md`.
It supersedes nothing in prior riders
(`2026-05-10-1400-deadreckon-build-rider.md`,
`2026-05-11-1400-deadreckon-primary-flow-rider.md`,
`2026-05-11-1400-deadreckon-robust-rider.md`,
`2026-05-11-1400-deadreckon-usability-rider.md`,
`2026-05-11-1444-deadreckon-orchestrate-rider.md`,
`2026-05-11-1502-deadreckon-codebase-rider.md`) — their invariants,
dependency policy, sandbox defaults, files-not-fields lineage pattern,
error-footer convention, and existing verbs still apply. This rider
adds three (or four) auto-generated doc artifacts per run, a doc-writer
provider hook, the `deadreckon doc` verb, and the cross-reference /
diff-coverage discipline borrowed from stoa.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided — do not redesign)

- **Maturity stays `alpha`.** Workspace stays `version = "0.1.0"`.
- **No `PipelineState` schema changes.** All doc state lives in
  files: `working/.deadreckon/docs/*.md`, `working/.deadreckon/docs/polish.json`
  (status of the polish pass), and a `doc_polish_hash` field added to
  `codebase.json` (not `state.json`) for idempotency.
- **The doc-writer is a provider, not a feature flag.** It can be the
  same provider as the run (default) or a cheaper override
  (`defaults.doc_provider` in `config.toml`).
- **No `git push`.** No remote calls from the binary. `apply` is the
  only thing that may mutate the user's checkout.
- **Failure of the polish pass is non-fatal.** Incremental templated
  narrative always survives; promotion proceeds. The run status
  remains `Completed`.
- **No V1 invention.** If a phase reveals a V1-architecture decision,
  log it in `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` and continue.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Stoa pattern reference (the target shape)

The rider's section names and frontmatter are not invented here —
they're cribbed verbatim from the impl-doc / AS-BUILT shape stoa has
been converging on. Key exemplars:

- `/Users/gdc/stoa/docs/implementation/2026-05-07-MEETING-AUTO-CAPTURE-AND-TRANSCRIPTION.md` —
  frontmatter (Date / Last updated / Status / Commit span / Owner),
  "Reading order" preamble, phase-by-phase "What shipped" body,
  mid-stream revision sections, "Updates since the spike" tail.
- `/Users/gdc/stoa/stoa-cli/pkg/scribe/AS-BUILT-ARCHITECTURE.md` —
  System overview, component table (file → role → entrypoint),
  process topology ASCII, wire protocols, file-system layout.
- `git -C /Users/gdc/stoa log -- docs/implementation/` — impl docs
  are LIVING DOCUMENTS, updated across many commits. "Doc parity" is
  a discipline (`docs(scribe): bring AS-BUILT + impl doc to parity with
  the second arc`, `chore(scribe): Phase 21 — search accuracy + doc
  parity`), not a one-shot.

The rider does not require the agent to read or imitate every stoa
example — just the two exemplar files above and the spirit of the
discipline.

## File layout (under `working/.deadreckon/docs/`)

```
working/.deadreckon/docs/
├── RUN-NARRATIVE.md         # impl-doc analogue
├── RUN-AS-BUILT.md          # subsystem-AS-BUILT analogue
├── RUN-DECISIONS.md         # design-doc-retrospective analogue
├── AS-BUILT-DELTA.md        # optional; only if source has AS-BUILT-ARCHITECTURE.md
├── _incremental.jsonl       # per-turn chunks before polish (templated, no LLM)
└── polish.json              # status of the polish pass
```

The polish prompt itself is **not** in the binary. It lives in the
`run-narrator` skill (see "Polish via the `run-narrator` skill"
below). The binary loads the skill, substitutes placeholders, and
sends one completion. This mirrors the Printing Press two-layer
split: judgment in markdown, invariants in Rust.

## Polish via the `run-narrator` skill

The polish prompt is a deadreckon skill — same mechanism as
`default-coding` (the existing skill at
`/Users/gdc/deadreckon/skills/default-coding/SKILL.md`). The binary
already loads skills by name via `PipelineState.skill_path`
(`state.rs:91-92`). This rider adds a second, doc-writing skill.

### Skill file location and resolution order

The binary resolves `<doc-skill-name>` (default `run-narrator`) at
polish time by checking the following paths in order; the first
match wins:

1. **Project override.** If `codebase.json.source_path` is set,
   `<source_path>/skills/<doc-skill-name>/SKILL.md`.
2. **User override.** `~/.deadreckon/skills/<doc-skill-name>/SKILL.md`
   (i.e., under `DEADRECKON_HOME`, alongside `config.toml`).
3. **Repo default.** `/Users/gdc/deadreckon/skills/<doc-skill-name>/SKILL.md`
   (ships with the binary).

If none exists, the polish pass is recorded as failed in
`polish.json` (status `no_skill`) and the incremental templated
narrative is promoted as-is. This is non-fatal — runs still complete.

### Skill file format

YAML frontmatter + Markdown body, matching the existing
`default-coding` skill shape:

```markdown
---
name: run-narrator
description: Produces RUN-NARRATIVE.md, RUN-AS-BUILT.md, RUN-DECISIONS.md, and (when applicable) AS-BUILT-DELTA.md from a deadreckon run's trace + diff. Output is a single JSON object.
output: json
---

# run-narrator

You are a doc writer producing implementation documentation in the
SpecStory/stoa style. The user just completed a deadreckon agentic
run. Use newsroom voice: short, specific, concrete. Cite each
non-obvious claim with [turn N].

## Inputs you'll receive

- Goal: `{{ goal }}`
- Run summary: `{{ run_summary }}`
- Diff (file → +adds/-dels): `{{ diff }}`
- Trace (compressed, ~30-turn budget): `{{ trace }}`
- Source AS-BUILT (if applicable): `{{ source_as_built }}`

## Produce

A single JSON object with these keys (all values are full Markdown
documents):

- `narrative` — full `RUN-NARRATIVE.md` content including frontmatter
- `as_built`  — full `RUN-AS-BUILT.md` content
- `decisions` — full `RUN-DECISIONS.md` content
- `delta`     — full `AS-BUILT-DELTA.md` content or empty string

## Constraints

- Frontmatter format is exact (see "Frontmatter (exact format)"
  section of the rider that ships with the binary; reproduce it
  literally).
- Coalesce turns into 3–8 phases; each phase ≤ 5 turns.
- Every non-frontmatter claim has a `[turn N]` citation.
- `delta` is empty unless the diff touched files near an existing
  `AS-BUILT-ARCHITECTURE.md` / `AS-BUILT.md`.
- No emojis.
- Newsroom voice; avoid LLM filler ("In conclusion", "It is worth
  noting", etc.).
```

### Placeholder substitution

The binary substitutes the following placeholders in the skill body
before sending the completion. Implemented as plain string
replacement; no expression language. Each placeholder appears
exactly once in the default skill; user overrides may add or remove
them.

| Placeholder | Source |
|---|---|
| `{{ goal }}` | `state.goal` verbatim |
| `{{ run_summary }}` | Composed: run-id, provider, turns, spend, commit span |
| `{{ diff }}` | `git diff --stat <base>..<head>` (worktree) or `inventory_files` summary (other modes) |
| `{{ trace }}` | First 200 chars of each turn's response + tool calls + outcome, capped at 30 turns (oldest middle-truncated) |
| `{{ source_as_built }}` | Contents of source `AS-BUILT-ARCHITECTURE.md` or empty (capped at 2000 chars + ellipsis) |

Unused placeholders are passed through (no error). Unknown
placeholders are passed through. The binary does not enforce a
placeholder schema beyond making each available — users can build
arbitrary skills.

### Config knob

```toml
[defaults]
doc_skill = "run-narrator"   # name; resolution via the three-tier order above
```

Per-run override: `deadreckon run "..." --doc-skill <name>` and
`deadreckon doc <id> --polish --doc-skill <name>`.

### Why a skill, not a const

- **Editable without rebuild.** Adjust house style in markdown.
- **Project-overridable.** A repo carries its own
  `skills/run-narrator/SKILL.md` that ships with the project.
- **Architecturally honest.** Matches the existing two-layer split
  the build rider established for `default-coding`.

### Alternate skills as future extensibility

The default `run-narrator` produces all four artifacts in one
completion. A future split (V1-CANDIDATE) might ship three skills —
`narrator`, `architect`, `decider` — for finer control. Out of
scope here; one skill ships in alpha.

After promotion these all land at
`/Users/gdc/.deadreckon/library/<scope>/<run-id>/docs/`.

For worktree-mode runs, the polish pass also writes a final commit on
the run's branch:
`docs: deadreckon RUN-NARRATIVE + AS-BUILT + DECISIONS for run <id-prefix>`
that adds these files. Copy-mode runs include them in the materialized
output. In-place runs land them under
`<source-path>/.deadreckon/docs/` (the same location as the snapshots).
Fresh runs put them in the runstate library entry only.

## Frontmatter (exact format — exercised by depth tests)

```markdown
# <Title — single line summarized from goal>

**Date:** <RFC3339 run start>
**Last updated:** <RFC3339 polish-pass timestamp; falls back to run end>
**Status:** <run.status> (alpha)
**Run ID:** `<uuid>`
**Goal:** <full goal verbatim, no truncation>
**Commit span:** `<base-sha-short>` … `<head-sha-short>` on `<branch>` (<N> turn commits, +<add>/-<del> LoC)
**Owner:** <git user.name> (with deadreckon + <provider-id>)
**Provider:** <provider-id>
**Sandbox:** <sandbox-kind>
**Spend:** $<X.YZ>  (or `<wall>s wall (subscription)` for cli:* providers)
**Doc-writer:** <doc-provider-id or "templated only">
```

Field order and the `**Bold:**` formatting match the stoa exemplar.
The depth tests parse this block and assert each field name + format.

For non-worktree modes, `Commit span` becomes
`Working dir: <path>` (copy) or `In-place: <path>` (in-place) or
omitted (fresh).

## `RUN-NARRATIVE.md` section schema

```markdown
<frontmatter block>

> **Reading order:** present only for extended runs; one-paragraph
> pointer to the parent narrative + the "Updates since" section
> below.

## Goal

<verbatim goal text, prose>

## Why now

<2-4 sentences, synthesized from the goal + any preamble; optional
for self-contained tasks>

## High-level approach

<single paragraph synthesized from the first 3 turns of trace —
what's the agent actually doing>

## What shipped in this run

### Phase 1 — <2-6 word title> (commit `<sha>`)

- <bulleted file changes with rationale>
- <key tool calls and their effects>
- <citations: [turn N](../traces.jsonl) for each claim>

### Phase 2 — <title> (commit `<sha>`)

...

## Updates since the parent run (only present for extended runs)

- <one bullet per material change vs parent's library>
- <links to specific diff vs parent>

## Open threads

- <TODOs left in code>
- <tests skipped>
- <"we punted on X" notes synthesized from final-turn context>
- <provider-window misses (truncated context) flagged>

## Cross-references

- Traces: `traces.jsonl` (N entries)
- Provenance: `provenance.jsonl` (N entries)
- Snapshots: `snapshots/turn-0/` … `snapshots/turn-N/`
- Branch: `<branch>` at `<head-sha>` (worktree mode)
- Acceptance: `proofs/turn-acceptance.json`
- Parent: `library/<scope>/<parent-id>/` (extend mode)
```

## `RUN-AS-BUILT.md` section schema

```markdown
<frontmatter block, with **Subject:** prefix line>

This document describes the subsystem changed by run <id-prefix>:
<one-paragraph subject summary>.

For the *why* and the chronological story of how we got here, see
[`RUN-NARRATIVE.md`](./RUN-NARRATIVE.md).

## System overview

<paragraph synthesized from diff>

## Components (changed in this run)

| Layer | Responsibilities | Key entrypoints |
| --- | --- | --- |
| ... | ... | ... |

## Process / data flow

```
<ASCII diagram synthesized by doc-writer when applicable; omitted
when the change is purely local edits>
```

## File-system layout (changed/added paths)

```
<tree-style listing of changed paths>
```

## External interactions

<list of external services / processes touched, if any>

## Cross-references

- Narrative: [`RUN-NARRATIVE.md`](./RUN-NARRATIVE.md)
- Decisions: [`RUN-DECISIONS.md`](./RUN-DECISIONS.md)
- Source AS-BUILT (if present): `<path>`
```

## `RUN-DECISIONS.md` section schema

```markdown
<frontmatter block>

This document captures the meaningful decisions made during run
<id-prefix>. Each entry corresponds to a trace turn where the agent
weighed alternatives.

## Decision N — <short title> (turn <N>)

**Considered:** <option A>, <option B>, <option C>
**Chose:** <option>
**Why:** <one-paragraph quoted reasoning excerpt from the LLM
response>
**Trace:** [turn N](../traces.jsonl)
**Files affected:** `<a/b.rs>`, `<c.rs>`

(Repeat per decision; if the run has no detected decisions, the file
is still written with a single line: "No multi-alternative decisions
detected in this run.")
```

## `AS-BUILT-DELTA.md` schema (conditional)

Generated only when:
1. `codebase.json.mode == "worktree"` AND the source git root has
   `AS-BUILT-ARCHITECTURE.md` or `AS-BUILT.md` at the top level OR in
   the same directory as any file in the run's diff;
2. AND the diff touched ≥ 3 files OR added a public function /
   exported type.

Format: stoa-shaped proposed amendments. The doc-writer is told to
produce ONLY diffs, not the full file. Each section of the existing
AS-BUILT that should change gets a `### Proposed amendment to
"<section>"` block with the new text. Each new section gets a
`### Proposed new section: "<title>"` block.

Marker: every block begins with `> deadreckon proposes (run
<id-prefix>):` so a human reviewer can grep.

When generated: written to `working/.deadreckon/docs/AS-BUILT-DELTA.md`.
The polish-pass commit also writes it as `docs/AS-BUILT-DELTA-<id-prefix>.md`
under the source-AS-BUILT's directory in worktree mode (so `apply`
brings the proposal to the user's checkout).

## Per-turn templating (no LLM call)

At the end of each successful turn, deadreckon appends one entry to
`_incremental.jsonl` and one section to a working `RUN-NARRATIVE.md`
draft. The section template:

```markdown
### Turn <N> — <auto-title> (commit `<sha>` or `-` if no commit)

- Tool: <tool-kind> (<latency>ms)
- Files: `<paths>`
- Outcome: <exit code / success / failure summary>
- Trace: [turn <N>](../traces.jsonl)
```

Auto-title generation (no LLM):
1. If LLM response contains the phrase `"I'll <verb> <noun>"`,
   extract the verb-noun (max 6 words). Else:
2. If exactly one tool was called, `<verb-by-tool-kind> <basename of
   first file>` (e.g., "Edit src/parser.rs", "Run cargo test",
   "Write hello.py").
3. Else, fall back to `Turn <N>` (no extracted title).

This stage is deterministic and produces narrative useful even if
the polish pass fails.

## End-of-run polish pass

Triggered on `RunStatus::Completed` before promotion. The binary:

1. Resolves the `doc_skill` (default `run-narrator`) via the
   three-tier order from the previous section.
2. Loads the skill, substitutes placeholders.
3. Sends one completion via `doc_provider` (defaults to the run's
   provider; override via `[defaults] doc_provider = "<id>"` in
   `config.toml`).
4. Parses the response as JSON.
5. Writes `narrative`/`as_built`/`decisions`/`delta` to
   `working/.deadreckon/docs/{RUN-NARRATIVE,RUN-AS-BUILT,RUN-DECISIONS,AS-BUILT-DELTA}.md`.
6. Records inputs hash + cost + provider + skill-source-path in
   `polish.json`.

If JSON parse fails, the binary retries once with `"Your last reply
was not valid JSON. Reproduce the JSON exactly."` On second failure,
the polish pass is recorded as failed (`polish.json.status =
"json_parse"`) and the incremental narrative is promoted as-is.

If the skill cannot be resolved (none of the three paths exists),
the polish pass is recorded as failed (`polish.json.status =
"no_skill"`) and the incremental narrative survives.

The default repo-shipped `run-narrator` skill IS the spec for what
the polish pass should produce. The content of that file (above, in
"Polish via the `run-narrator` skill") is what the binary ships.

### Idempotency

`polish.json` carries a SHA-256 hash of the inputs (goal + trace
content + diff content + source AS-BUILT). If `deadreckon doc <id>
--polish` is invoked again with the same hash, no new call is made.

### Cost

Typical run: 8–20K output tokens; with a $3/$15 model, ~$0.05–$0.30
per polish. Users can swap to a cheaper `doc_provider` (e.g.,
`openai:gpt-4o-mini`, `cli:claude-code-haiku`) for less.

## Phase detection (for the per-turn templated narrative)

Used in P3 to coalesce the per-turn `_incremental.jsonl` into 3–8
phases for the templated narrative when the polish pass is disabled
or fails.

Algorithm:

```
groups = []
current = [turn_0]
for turn in turns[1:]:
    if same_file_overlap(current, turn) > 0.5
       OR same_tool_kind_consecutive_count(current) < 3:
        current.append(turn)
    else:
        groups.append(current); current = [turn]
groups.append(current)
if len(groups) > 8:
    coalesce_smallest_neighbors_until_8(groups)
```

Each group → phase. Phase title comes from the first turn's
auto-title.

## Decision detection (for `RUN-DECISIONS.md` and the polish hint)

Used in P4. Regex over each turn's LLM response text:

```
let DECISION_MARKERS = [
    r"(?i)\b(let me consider|let me think|i'll go with|i'll choose)\b",
    r"(?i)\b(option [123]|alternatives?:|either .* or)\b",
    r"(?i)\b(instead of|rather than|actually,?\s*let)\b",
    r"(?i)\bdecision\b.*\b(chose|pick|go(?:ing)? with)\b",
];
```

A turn matches a decision if it hits ≥ 1 marker AND the response is
≥ 200 chars (filters out trivial mentions). Matched turns are
flagged in `_incremental.jsonl` as `decision_candidate: true`. The
polish pass extracts the considered/chosen/why; the templated
fallback emits a `Decision N` entry with the response text as `Why:`.

## Cross-link convention

Every reference uses relative paths from the doc file's location:

- From `working/.deadreckon/docs/RUN-NARRATIVE.md` to
  `working/traces.jsonl` → `[turn N](../traces.jsonl)`. (Note:
  `traces.jsonl` doesn't have anchor support; the link points to the
  file. The "turn N" text is the human cue.)
- Cross-doc: `RUN-NARRATIVE.md` ↔ `RUN-AS-BUILT.md` ↔
  `RUN-DECISIONS.md` use simple `./<name>.md`.
- After promotion, the same relative paths still resolve because the
  entire `working/` tree is what gets renamed into `library/<scope>/<run-id>/`.

## Diff coverage check + polish retry

After polish completes (P7):

1. Compute `git diff --name-only <base>..<head>` for worktree mode,
   or `inventory_files` for copy/in-place modes.
2. For each file in the diff, search `RUN-NARRATIVE.md` for the
   filename (basename match; relative path also matches). Build the
   missing-files set.
3. If missing-files is non-empty AND polish retries < 2:
   - Re-invoke the polish call with an additional instruction:
     `Your previous output omitted these files; revise to include
     them with citations: <list>`.
   - Increment retry count.
4. If still missing after 2 retries, log a warning to traces and
   proceed. The run is not failed.

## `apply` commit body integration

When `deadreckon apply <run-id>` builds the squash commit (per the
codebase-rider), the default body is:

```
<goal-first-line> (deadreckon run <run-id-prefix>)

<executive-summary-paragraph-from-RUN-NARRATIVE-or-first-paragraph-under-High-level-approach>

Phases:
- Phase 1: <title> (turns N-M, +<add>/-<del>)
- Phase 2: ...

Decisions: <count> (see docs/RUN-DECISIONS.md)
Open threads: <count> (see docs/RUN-NARRATIVE.md#open-threads)

Generated by deadreckon. Trace: docs/RUN-NARRATIVE.md
```

`--message` still overrides the whole body.

## `deadreckon doc` verb

```
deadreckon doc <run-id>
    [--kind narrative|as-built|decisions|delta]   # default: narrative
    [--export <path>]                              # write to path instead of stdout
    [--polish]                                     # force fresh polish call
    [--no-confirm]                                 # used with --polish to skip cost confirm
```

Behavior:
- Default: read `library/<scope>/<run-id>/docs/<kind>.md` and print to
  stdout. If not yet promoted, read from `working/.deadreckon/docs/`.
- `--export <path>`: copies to the path. Refuses if dest exists
  unless `--force` (which is added to the verb spec).
- `--polish`: triggers a fresh polish call regardless of cache.
  Confirms cost (`this will cost ~$<estimate>; continue? [y/N]`)
  unless `--no-confirm`.

Refusal cases:
| Condition | Error | Try |
|---|---|---|
| Run not found | `no run <id>` | `deadreckon list` |
| Run not completed | `run <id> is <status>; docs are not yet polished` | `deadreckon resume <id>` or `--kind narrative` for incremental |
| Kind == `delta` but file absent | `no delta produced; this run did not affect a project AS-BUILT` | (none) |
| `--polish` and no `doc_provider` configured | `no doc provider configured` | `deadreckon init or set defaults.doc_provider` |
| `--export <path>` exists | `dest <path> exists` | `--force or pick a fresh path` |

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them
fail; implement; green on
`cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`;
conventional-commit local commit; CHANGELOG entry.

### P1 — Doc directory + frontmatter helpers

- New module `crates/deadreckon-core/src/docs.rs` with
  `FrontmatterFields` struct, `frontmatter(&PipelineState, ...) ->
  String`, file path helpers (`docs_dir(working)`, `narrative_path`,
  `as_built_path`, `decisions_path`, `delta_path`).
- `working/.deadreckon/docs/` created at run start; skeleton
  `RUN-NARRATIVE.md` with frontmatter written at turn 0.

Depth tests (in `crates/deadreckon/tests/self_documenting.rs`):
- `docs_dir_created_at_run_start`
- `frontmatter_contains_required_fields_in_order`
- `frontmatter_handles_subscription_provider_format`
- `frontmatter_omits_commit_span_for_fresh_mode`

### P2 — Per-turn narrative chunks (templated, no LLM)

- After each successful turn, append a turn-section to
  `RUN-NARRATIVE.md` and a JSON record to `_incremental.jsonl`.
- Auto-title via the rider's three-step rule.

Depth tests:
- `three_turn_run_produces_three_turn_sections`
- `each_turn_section_has_required_fields`
- `auto_title_from_ill_verb_noun_phrase`
- `auto_title_fallback_to_tool_plus_basename`
- `commit_sha_present_in_worktree_mode_blank_otherwise`

### P3 — Phase detection + coalescing

- Implement the rider's algorithm; expose as
  `coalesce_into_phases(&[TurnRecord]) -> Vec<Phase>` in
  `docs.rs`.
- Templated narrative groups turn-sections under phase headers.

Depth tests:
- `twelve_turns_collapse_to_three_to_eight_phases`
- `same_file_turns_coalesce`
- `tool_kind_changes_break_phase`
- `phase_titles_come_from_first_turn`

### P4 — Decision detection

- Regex helpers in `docs.rs`; flag each turn with
  `decision_candidate: bool` in `_incremental.jsonl`.
- Templated `RUN-DECISIONS.md` emitted from flags + response text.

Depth tests:
- `decision_markers_detected_case_insensitive`
- `short_response_below_200_chars_not_a_decision`
- `templated_decisions_md_lists_each_decision_with_turn_link`
- `no_decisions_emits_single_line_no_decisions_message`

### P5 — End-of-run polish pass via the `run-narrator` skill

- Ship the default skill at
  `/Users/gdc/deadreckon/skills/run-narrator/SKILL.md` (contents per
  "Polish via the `run-narrator` skill" section above).
- New module `crates/deadreckon-core/src/polish.rs` with
  `resolve_skill(name, paths) -> Result<PathBuf>` (three-tier
  resolution) and `polish_docs(skill_path, provider, inputs) ->
  Result<PolishedDocs>`.
- Placeholder substitution: plain string replacement; pass-through
  for unknown placeholders.
- Single LLM call; JSON parsed; retry once on malformed JSON.
- `polish.json` records inputs hash, provider, skill source path,
  completion time, cost, status.
- Skill-not-resolvable → `status: "no_skill"`, run still succeeds.
- JSON-unparseable → `status: "json_parse"`, run still succeeds.

Depth tests:
- `polish_runs_once_on_completion`
- `polish_idempotent_on_same_input_hash` (mocked provider; count
  invocations)
- `polish_failure_does_not_fail_run`
- `polish_json_retry_on_malformed_first_response`
- `polish_uses_doc_provider_override_when_configured`
- `polish_resolves_project_skill_before_user_before_repo`
- `polish_records_no_skill_status_when_unresolvable`
- `placeholder_substitution_replaces_known_handles_unknown_passthrough`

### P6 — `AS-BUILT-DELTA.md` detection + generation

- Detection per the rider's trigger.
- In worktree mode, the polish pass writes
  `docs/AS-BUILT-DELTA-<id-prefix>.md` to the branch as a final commit
  so `apply` brings it to the user's checkout.

Depth tests:
- `delta_emitted_when_source_has_as_built_at_root`
- `delta_emitted_when_diff_touches_as_built_neighbor`
- `delta_skipped_when_no_source_as_built`
- `delta_skipped_when_diff_under_three_files`
- `delta_commit_lands_on_branch_in_worktree_mode`

### P7 — Diff coverage check + polish retry

- Post-polish coverage check per the rider.
- Polish-retry path with the additional-instruction prompt augment.

Depth tests:
- `diff_coverage_check_passes_when_all_files_named`
- `missing_file_triggers_polish_retry`
- `still_missing_after_two_retries_logs_warning_but_promotes`

### P8 — `apply` commit body integration

- Hook into `deadreckon apply` (from the codebase-rider) to read
  `RUN-NARRATIVE.md` + extract executive summary + phase list.
- Body template per the rider.

Depth tests:
- `apply_commit_body_contains_executive_summary`
- `apply_commit_body_lists_phases`
- `apply_commit_body_links_to_run_narrative`
- `apply_message_flag_overrides_body`

### P9 — `deadreckon doc` verb

- See verb signature above.

Depth tests:
- `doc_default_prints_narrative`
- `doc_kind_as_built_prints_as_built`
- `doc_kind_decisions_prints_decisions`
- `doc_kind_delta_prints_or_says_no_delta`
- `doc_export_writes_to_path`
- `doc_export_refuses_existing_path_unless_force`
- `doc_polish_triggers_fresh_call_with_confirm`

### P10 — Extend / orchestrate / library integration

- **Extend**: new run's `RUN-NARRATIVE.md` frontmatter includes
  `**Parent run:** <parent-id-prefix>` line; "Reading order"
  paragraph references parent's library narrative; an "Updates since"
  section is appended at the parent's narrative pre-promotion (this is
  the doc-parity habit).
- **Orchestrate** (if landed): a `PLAN-NARRATIVE.md` per plan in
  `~/.deadreckon/plans/<plan-id>/docs/` aggregates each child's
  executive summary. Polish call reuses the same provider.
- **`deadreckon list`** gains a `DOCS` column:
  `polished` / `incremental` / `failed` / `n/a`.

Depth tests:
- `extend_narrative_links_to_parent`
- `extend_updates_parent_narrative_with_updates_since`
- `plan_narrative_aggregates_child_summaries` (gated on orchestrate
  having landed)
- `list_shows_docs_status_column`

### P11 — AS-BUILT update + CHANGELOG

- Insert new top-level section into
  `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:

  ```
  ## NN. Self-Documenting Runs

  NN.1 The three artifacts (narrative / as-built / decisions)
  NN.2 Frontmatter (exact format)
  NN.3 Per-turn templating (no LLM)
  NN.4 End-of-run polish pass (one LLM call)
  NN.5 Phase + decision detection
  NN.6 Diff coverage + polish retry
  NN.7 AS-BUILT-DELTA (conditional)
  NN.8 apply commit body integration
  NN.9 deadreckon doc verb
  NN.10 Cost & idempotency
  ```

  `NN` = next available top-level number (after orchestrate-§18 and
  codebase-§19 land, this becomes §20).

- Update §22 (current "What's Built vs Scaffolding-Thin"):
  - **Add to "Built and reliable":** self-documenting docs in stoa
    shape, polish pass with retry, diff coverage check, `deadreckon
    doc` verb, AS-BUILT-DELTA, apply commit-body integration.
  - **Note explicitly:** this rider does **not** close any prior §22
    thin items; it adds capability. The thin list is unchanged.

- Append to `/Users/gdc/deadreckon/docs/CHANGELOG.md`:

  ```
  ## Self-documenting runs (alpha) — <YYYY-MM-DD>

  - Every run now produces `RUN-NARRATIVE.md`, `RUN-AS-BUILT.md`,
    `RUN-DECISIONS.md` under `working/.deadreckon/docs/` (in stoa
    shape; matches the impl-doc + per-subsystem AS-BUILT pattern
    mined from /Users/gdc/stoa).
  - Optional `AS-BUILT-DELTA.md` when the source carries its own
    AS-BUILT-ARCHITECTURE.md.
  - Polish prompt is the new `run-narrator` skill at
    `skills/run-narrator/SKILL.md`; per-user and per-project overrides
    via the three-tier resolution order; configurable via
    `defaults.doc_skill` and `--doc-skill`.
  - `deadreckon doc <run-id>` verb to read / export / re-polish.
  - End-of-run polish via configurable `doc_provider` (single
    completion; idempotent; non-fatal on failure).
  - Diff-coverage check with up to 2 retries on missing files.
  - `apply` commit body now includes the executive summary + phase
    list.
  - Extend carries narrative lineage forward; the parent narrative
    gets an "Updates since" section.
  ```

## Integration matrix

| Mode | Doc location during run | Doc location after promotion | apply impact |
|---|---|---|---|
| worktree | `working/.deadreckon/docs/` (worktree path) | `library/<scope>/<id>/docs/` + final commit on branch | commit body includes summary + phases |
| copy | `working/.deadreckon/docs/` (runstate) | `library/<scope>/<id>/docs/` | n/a (no apply) |
| in-place | `<source>/.deadreckon/docs/` | `library/<scope>/<id>/docs/` | n/a |
| fresh | `working/.deadreckon/docs/` (runstate) | `library/<scope>/<id>/docs/` | n/a |

## Config additions (`config.toml`)

```toml
[defaults]
# Existing keys (provider, sandbox, max_spend, ...) stay.
doc_skill = "run-narrator"              # name; three-tier resolution
doc_provider = "openai:gpt-4o-mini"     # falls back to default provider when absent
doc_polish_retries = 1                  # JSON-malformed retry; default 1
doc_diff_coverage_retries = 2           # missing-file retries; default 2
no_docs = false                         # global off-switch; --no-docs is per-run
```

Per-run flag override: `--doc-skill <name>` on `deadreckon run` and
`deadreckon doc <id> --polish`.

## Out of scope

- **Cross-run doc rollups.** A "what shipped in the last 7 days
  across all runs" report is V1.
- **HTML export.** `doc --export` writes markdown only.
- **Multi-language doc-writer prompts.** English only.
- **In-flight live narrative streaming.** The incremental narrative
  updates per turn but is not streamed event-by-event to the TUI;
  the TUI shows it via a `D` keybind (V1-CANDIDATE).
- **Auto-PR.** No automatic PR-opening with the docs attached;
  `apply` lands on the user's checkout and that's it.
- **Doc lints.** No "your narrative is missing a 'Why now' section"
  enforcement; the polish prompt nudges but doesn't gate.
- **Cross-reference to issue trackers.** No Jira/Linear/GitHub Issue
  hydration in the narrative; that's V1.
- **Doc-only re-runs.** `deadreckon doc --polish` re-polishes; there
  is no separate `deadreckon redoc` verb.

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes.** All state in files
  (`polish.json`, `_incremental.jsonl`, the docs themselves).
- **One depth test before each phase implementation.** A phase whose
  tests all started green never failed; that's a smell.
- **Polish failure is non-fatal.** Promotion proceeds with the
  incremental narrative when polish fails.
- **No silent expansion.** Anything beyond the eleven phases above
  goes into `V1-CANDIDATES.md`.
- **The `run-narrator` skill is the spec.** The repo-shipped skill
  at `/Users/gdc/deadreckon/skills/run-narrator/SKILL.md` is the
  canonical polish prompt. User and project overrides are first-class
  (judgment in markdown is the printing-press contract). Depth tests
  cover skill resolution + the default skill's structure, but do not
  pin the default's full text — that's free to evolve.
- **Frontmatter format is the spec.** The exact field order and
  `**Bold:**` formatting is depth-tested.

## Dependencies (per Tier 1/2/3 policy)

Tier 1 (utility, free):
- `regex` — already approved in orchestrate-rider for `history grep`;
  reused here for decision detection.
- No others expected.

Tier 2 (architectural): none.

Tier 3 (blocked): same blocks as prior riders.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with the relevant depth tests passing and a
  CHANGELOG entry naming the SHA.
- After P11, run a smoke flow end-to-end (preferably a real run
  against this repo with `--smoke` then a worktree run against a
  toy fixture) and capture the asciinema cast at
  `/Users/gdc/deadreckon/demo-self-documenting.cast`.
- If a phase reveals a V1-architecture decision, stop and log it in
  `V1-CANDIDATES.md`; do not silently expand scope.
