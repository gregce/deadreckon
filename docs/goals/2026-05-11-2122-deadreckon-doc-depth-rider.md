# deadreckon — Doc Depth Rider (close the gap to stoa shape)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-11-2122-deadreckon-doc-depth-goal.md`.
It supersedes nothing in prior riders — and most directly extends
`/Users/gdc/deadreckon/docs/goals/2026-05-11-1525-deadreckon-self-documenting-rider.md`
(predecessor). Their invariants, file layouts, three-tier skill
resolution, polish-failure-non-fatal posture, frontmatter format,
diff-coverage retry, error-footer convention, and existing verbs
still apply. This rider adds: richer per-turn capture, fixed
templated rendering, a multi-section skill split, auto-detected
doc_provider, and stoa-shape prompts. The rider does not change
`PipelineState` and does not introduce any new top-level CLI verbs
(only flags + sub-skills).

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided — do not redesign)

- **Maturity stays `alpha`.** Workspace stays `version = "0.1.0"`.
- **No `PipelineState` schema changes.** All new state lives in
  files: richer per-turn JSONL, the four sub-skill SKILL.md files,
  and `polish.json` extended with sub-call records.
- **Predecessor's polish pipeline is the substrate.** This rider
  splits the single polish call into four sub-calls and richens the
  inputs; the file layout (`working/.deadreckon/docs/...`),
  resolution order, idempotency hashing, and non-fatal failure are
  unchanged.
- **No new top-level verb.** All new behavior is reachable via
  existing `deadreckon doc <id> [--polish] [--force]`,
  `deadreckon run "..." [--no-docs] [--doc-skill]`, and
  `deadreckon config`.
- **Backwards-compatible.** A user with a single legacy
  `skills/run-narrator/SKILL.md` keeps it working: when no sub-skills
  are present, the binary falls back to a single call against the
  legacy skill (current behavior). The split is the default for
  fresh installs.
- **Failure stays non-fatal.** Any sub-skill failure flips that
  doc's status in `polish.json` to `failed_subcall:<name>`; the
  templated draft survives; the run still completes.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** If a phase reveals a major architectural
  decision, log it in `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`
  and continue.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## The depth gap, evidence-grounded

Concrete comparison against
`/Users/gdc/test-deadreckon/build-a-full-ms-paint-ty/docs/`:

| Dimension | Today (ms-paint, 60-line `RUN-NARRATIVE.md`) | Stoa target |
|---|---|---|
| Length | 60 lines | 200–700+ lines |
| Title | Truncated at column ~40: `# make it possible to create and add a gallery of` | Full goal, no truncation |
| "High-level approach" | `deadreckon progressed through turn 1 used cli_subagent and tracked the resulting files, traces, snapshots, and provenance.` | Synthesized paragraph naming the actual approach |
| Phase prose | `Turns 1-1 changed README.md, app.js, index.html, styles.css.` | One paragraph per phase (intent → actions → result), with file:line citations |
| Per-turn outcome | `Outcome: Implemented artwork gallery support.  Changed files: - [index.html](/.../index.html:162):...` (truncated mid-sentence) | Full outcome paragraph + a 5-line diff sample of the largest hunk |
| Components table | `Documentation`, `Frontend/runtime`, `Project files` (generic, derived from extension only) | Per-component row with **layer + responsibilities + key entrypoints (file:line)** |
| Process topology | `Goal -> provider turn -> tool call -> snapshot/provenance -> docs -> gate -> promotion` (generic deadreckon flow) | Multi-process ASCII with fd numbers / IPC / external services |
| Decisions doc | `No multi-alternative decisions detected.` | Either real decisions per turn or an honest "no decisions" with the regex marker recap |
| Doc-writer line | `Doc-writer: templated only` | `Doc-writer: cli:codex (sub-skills: overview, phases, as-built, decisions)` |

Root causes (each maps to a phase below):

1. Polish pass never ran — `doc_provider` defaulted to nothing.
   (P2: auto-resolve to subscription CLI provider.)
2. Per-turn input was capped at 200 chars / no diff samples / no
   tool stdout — there's nothing for the LLM to chew on. (P1.)
3. Skill prompt is shallow; no ask for components table prose,
   topology, "load-bearing"/"seams", reading-order preamble. (P5.)
4. Single polish call + 8 K token budget can't produce 700-line
   docs. (P6: split into 4 sub-calls × 16 K.)
5. Templated narrative is robotic by design. (P3: phase paragraphs.)
6. Component table maps file extension to layer name. (P4: path-rule
   inference with citations.)

## Data model (files-not-fields)

### `_incremental.jsonl` — per-turn schema (extended)

Today's schema (predecessor):

```json
{
  "turn": 1,
  "tool_kind": "cli_subagent",
  "latency_ms": 454601,
  "files": ["README.md", "app.js"],
  "outcome": "Implemented artwork ... (truncated at 200 chars)",
  "trace_pointer": "../traces.jsonl#turn-1",
  "snapshot_pointer": "../../snapshots/turn-1/",
  "commit_sha": "e48a655",
  "decision_candidate": false,
  "auto_title": "Edit README.md"
}
```

Extended schema (this rider):

```json
{
  "turn": 1,
  "tool_kind": "cli_subagent",
  "latency_ms": 454601,
  "files": [
    {
      "path": "README.md",
      "adds": 14,
      "dels": 0,
      "largest_hunk_excerpt": "+## Gallery\n+\n+Browse saved artwork at /gallery.\n+...",
      "is_new": false,
      "is_binary": false
    }
  ],
  "response_full": "<full provider response, capped 50 KB>",
  "response_summary": "<first paragraph or synthesized 1-line>",
  "tool_stdout": "<bash stdout, capped 10 KB; absent for non-bash>",
  "tool_stderr": "<bash stderr, capped 10 KB; absent for non-bash>",
  "trace_pointer": "../traces.jsonl#turn-1",
  "snapshot_pointer": "../../snapshots/turn-1/",
  "commit_sha": "e48a655",
  "decision_candidate": false,
  "auto_title": "Edit README.md"
}
```

`response_summary` rule: take the first paragraph of `response_full`
(up to first blank line), trim to 280 chars, end on a word boundary.

`largest_hunk_excerpt` rule: parse `git diff <snapshot N-1>..<snapshot N> -- <file>`
(or `git diff --no-index` for non-git working dirs); take the hunk
with the most `+`/`-` lines; emit up to 5 lines including the `@@`
header.

### `polish.json` — extended

Today's schema (predecessor): `inputs_hash`, `provider`,
`skill_source_path`, `completed_at`, `cost_usd`, `status`.

Extended:

```json
{
  "schema_version": 2,
  "inputs_hash": "<sha256>",
  "provider": "cli:codex",
  "doc_provider_source": "config|auto_subscription|flag",
  "subcalls": [
    {"skill": "narrator-overview", "skill_source_path": "/path/SKILL.md", "status": "ok", "tokens_in": 4231, "tokens_out": 1402, "cost_usd": 0.0, "duration_ms": 4210},
    {"skill": "narrator-phases", ...},
    {"skill": "narrator-as-built", ...},
    {"skill": "narrator-decisions", ...}
  ],
  "merged_at": "2026-05-11T20:01:00Z",
  "diff_coverage": {"missing_files": [], "retries": 0},
  "status": "ok|failed_subcall:<name>|json_parse|no_skill"
}
```

`schema_version: 1` polish files (predecessor's shape) are read
silently — the binary upgrades them to `2` on next polish.

### Skill files — four sub-skills

Each lives at `skills/<name>/SKILL.md` in the repo, with the same
three-tier resolution as `run-narrator`:

```
skills/
├── narrator-overview/SKILL.md      # owns RUN-NARRATIVE intro + closing
├── narrator-phases/SKILL.md        # owns the per-phase prose body
├── narrator-as-built/SKILL.md      # owns RUN-AS-BUILT.md
├── narrator-decisions/SKILL.md     # owns RUN-DECISIONS.md
└── run-narrator/SKILL.md           # legacy fallback (kept for back-compat)
```

User and project overrides land at the same per-name paths under
`~/.deadreckon/skills/` and `<source>/skills/`.

The legacy `run-narrator/SKILL.md` is **kept**; if any sub-skill
fails to resolve, the binary falls back to the legacy single-call
path for the missing piece (so partial-override is supported).

## Sub-skill prompt prescriptions (the spec)

Each sub-skill ships with the YAML/Markdown shape established by
predecessor (`name`, `description`, `output: json`). The body
of each sub-skill MUST ask for the following sections.

### `narrator-overview`

Output JSON: `{ overview, reading_order, why_now, high_level_approach, open_threads, cross_references }`.

The prompt explicitly asks for:

- **`reading_order`** — one paragraph (omit on solo runs; emit only
  for extended runs with a `parent_narrative`).
- **`why_now`** — 2–4 sentences synthesizing the goal and any
  preamble. Newsroom voice, no LLM filler.
- **`high_level_approach`** — one paragraph naming the actual
  approach taken (parsed from `response_full` + `tool_stdout`),
  including any pivot or supersession.
- **`open_threads`** — bulleted list of TODOs/punted items detected
  in `response_full` or trace; the prompt asks the model to scan for
  phrases like "left for follow-up", "out of scope", "TODO",
  "noted but not implemented".
- **`cross_references`** — paths to traces, provenance, snapshots,
  branch SHA, parent library, acceptance.

### `narrator-phases`

Output JSON: `{ phases: [{title, commit_sha, prose, file_changes, citations}] }`.

For each phase the prompt asks for:

- **`title`** — 2–6 words; reuses templated `auto_title` when no
  better synthesis is possible.
- **`prose`** — one paragraph per phase: what the agent intended,
  what tool calls it made, what changed, what the outcome was. Cite
  `[turn N]` for every non-frontmatter claim. **Quote a 1–3 line
  excerpt** from the largest diff hunk inline.
- **`file_changes`** — per-file rows with `+adds/-dels` and a 1-line
  description of what changed in that file.
- **`citations`** — list of trace/snapshot links for the phase.

### `narrator-as-built`

Output JSON: `{ subject, system_overview, components, topology, file_layout, external_interactions, cross_references }`.

The prompt requires:

- **`components`** — markdown table: `Layer | Responsibilities | Key entrypoints`. **Every row** must have a file:line in the entrypoints column. The skill is told that "Project files" is forbidden; if it can't infer a layer, it must omit the row rather than emit a generic one.
- **`topology`** — fenced ASCII block showing process / data flow when applicable. The skill is told to omit rather than emit a generic placeholder. The seed topology from P4 (path-derived) is provided as `{{ source_layout }}` for the skill to refine.
- **`system_overview`** — one paragraph naming what subsystem the run touched and why it matters; explicitly call out **"What's load-bearing"** and **"Where the seams are"** in two short sub-paragraphs.
- **`external_interactions`** — services / files / processes touched outside the working dir.
- **`cross_references`** — links to RUN-NARRATIVE, RUN-DECISIONS, source AS-BUILT.

### `narrator-decisions`

Output JSON: `{ decisions: [{title, turn, considered, chosen, why, files_affected, citations}] }` or `{ decisions: [] }`.

The prompt requires the model to either return an empty list or
return ≥1 entry per turn flagged `decision_candidate: true`. If the
flagged turn doesn't actually contain a decision, the model is told
to omit it (false-positive filtering). For empty lists, the binary
emits the canonical "No multi-alternative decisions detected" line.

## Doc-provider auto-resolution rules

Order of resolution (P2):

1. `--doc-provider` CLI flag, if set.
2. `[defaults] doc_provider` in `config.toml`, if set.
3. **First in-PATH subscription CLI provider** (`cli:codex`, then
   `cli:claude-code`).
4. **Run's own provider** (current behavior — what
   predecessor falls back to when nothing else is set).
5. None — record `polish.json.status = "no_provider"` and skip
   polish; templated draft survives.

The decision is recorded in `polish.json.doc_provider_source`
(`flag` / `config` / `auto_subscription` / `run_provider` / `none`).

The error footer when no provider is available:

```
deadreckon: no doc provider available
try: deadreckon config set defaults.doc_provider cli:codex
try: install codex (https://github.com/openai/codex) — auto-detected on next run
```

## Component-table inference rules (P4)

When the skill is unavailable, the templated path generates the
components table from the changed files using a closed mapping. The
rules are:

| Path pattern | Layer column |
|---|---|
| `crates/<name>/` | `Crate <name> (Rust)` |
| `Cargo.toml`, `Cargo.lock` | `Workspace manifest` |
| `src/components/<X>/` | `Frontend component (<X>)` |
| `src/pages/`, `src/routes/`, `app/`, `pages/` | `Frontend route` |
| `**/*.test.*`, `tests/`, `__tests__/` | `Tests` |
| `docs/`, `*.md` (root) | `Documentation` |
| `migrations/`, `**/*.sql` | `Database migration` |
| `.github/workflows/` | `CI` |
| `Makefile`, `Justfile` | `Build script` |
| `package.json`, `pnpm-lock.yaml`, `yarn.lock` | `Frontend manifest` |
| `pyproject.toml`, `requirements*.txt` | `Python manifest` |
| `go.mod`, `go.sum` | `Go module` |
| anything else | (omitted — `Project files` is **never** emitted) |

For matched rows, the entrypoint column is `<file>:<line-of-largest-change>`
when known, else `<file>`.

The polish call receives this seed as `{{ source_layout }}` and is
asked to refine (add prose, merge near-duplicate rows). The seed is
the floor; the polish output replaces it.

## Process-topology generation (P4)

Trigger: ≥ 3 distinct top-level directories under the working dir
appear in the diff. Examples that trigger: `crates/`, `skills/`,
`docs/`. Single-directory edits (e.g. only `docs/`) skip the
topology and the section is omitted from RUN-AS-BUILT.

Default ASCII (templated; the polish call can refine):

```
+-----------+   +-----------+   +-----------+
| crates/   |-->| skills/   |   | docs/     |
+-----------+   +-----------+   +-----------+
     |                                ^
     +--------------------------------+
```

Edges are derived by `grep -rE "<other-dir>/" <dir>` for each pair
of changed top-level dirs; an edge is drawn if any cross-reference
exists. Exact ASCII glyphs (`+`, `-`, `|`, `>`, `^`) are pinned by
depth tests.

## Verb signatures (no new top-level verbs)

Only flag/behavior changes:

```
deadreckon run "<goal>"
    [--doc-provider <id>]              # P2: per-run override
    [--no-docs]                        # existing
    [--doc-skill <name>]               # existing
deadreckon doc <id>
    [--polish]                         # existing
    [--force]                          # P9: ignore inputs hash
    [--no-confirm]                     # existing
    [--budget-cap <usd>]               # P6: refuse polish above $X
deadreckon config set defaults.doc_provider <id>   # existing
deadreckon config set defaults.doc_subskills <list>  # P5: comma-separated
```

Refusal cases:

| Condition | Error | `try:` |
|---|---|---|
| No doc_provider resolvable | `no doc provider available` | `deadreckon config set defaults.doc_provider cli:codex` |
| Sub-skill resolves but JSON malformed twice | `polish sub-skill '<name>' returned malformed JSON twice` | `deadreckon doc <id> --polish --force --doc-skill <name>` |
| `--budget-cap` exceeded | `polish would cost ~$<X>; cap is $<Y>` | `deadreckon doc <id> --polish --budget-cap <X>` or `--no-confirm` |
| All sub-skills fail | `polish failed; templated drafts survive` | `deadreckon doc <id> --polish --force` |

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them
fail; implement; green on
`cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`;
conventional-commit local commit; one-line CHANGELOG entry.

### P1 — Per-turn capture richness

- Extend `_incremental.jsonl` schema per "Data model" above.
- New helpers in `crates/deadreckon-core/src/docs.rs`:
  `capture_response_full`, `capture_response_summary`,
  `capture_diff_samples`, `capture_tool_stdio`.
- `turn_loop.rs` calls these after every successful turn (cli_subagent
  + bash + write_file paths) and before `append_turn_doc`.
- Caps: response_full 50 KB, tool_stdout/stderr 10 KB each,
  hunk excerpt 5 lines.

Depth tests (in `crates/deadreckon/tests/doc_depth.rs`):
- `incremental_jsonl_carries_full_response_up_to_50kb`
- `incremental_jsonl_carries_diff_samples_per_file`
- `incremental_jsonl_carries_bash_stdout_and_stderr`
- `response_summary_ends_on_word_boundary`
- `diff_sample_picks_largest_hunk_with_at_header`
- `binary_files_marked_is_binary_no_excerpt`

### P2 — Doc-provider auto-resolution

- Implement the resolution order in §"Doc-provider auto-resolution".
- Record `doc_provider_source` in `polish.json`.
- Update the no-provider error footer per the rider.

Depth tests:
- `doc_provider_resolves_from_flag_first`
- `doc_provider_resolves_from_config_second`
- `doc_provider_resolves_to_cli_codex_when_in_path`
- `doc_provider_resolves_to_cli_claude_code_when_codex_absent`
- `doc_provider_falls_back_to_run_provider_last`
- `doc_provider_records_source_in_polish_json`
- `no_doc_provider_emits_install_try_hint`

### P3 — Templated narrative prose upgrade

- Fix `# heading` truncation: use full goal, no character cap.
- Replace robotic "deadreckon progressed through turn N used X"
  prose with synthesized one-paragraph approach derived from
  per-turn `response_summary`s.
- Per-phase: emit one paragraph per phase combining
  `response_summary`s of contained turns, plus a per-file
  `+adds/-dels` list and a 3-line largest-hunk inline.
- "Open threads" templated extraction: regex over
  `response_full`s for `TODO`, `out of scope`, `follow-up`,
  `noted but not implemented`.

Depth tests:
- `narrative_heading_uses_full_goal_no_truncation`
- `phase_paragraph_combines_response_summaries`
- `per_file_adds_dels_appear_in_phase_body`
- `largest_hunk_excerpt_inlined_in_phase_body`
- `open_threads_extracted_from_todo_phrases`
- `outcome_text_never_truncated_at_200_chars`

### P4 — Component-table inference + topology generation

- Implement the path-rule mapping in §"Component-table inference
  rules"; emit the seed table for `narrator-as-built`.
- Implement the topology generator in §"Process-topology generation".
- Generic `Project files` row is **never** emitted by the templater.

Depth tests:
- `crates_path_maps_to_crate_layer_with_name`
- `frontend_components_path_maps_to_frontend_component`
- `tests_path_maps_to_tests_layer`
- `docs_path_maps_to_documentation_layer`
- `unmapped_path_omitted_not_emitted_as_project_files`
- `topology_emitted_only_when_three_or_more_top_dirs`
- `topology_arrows_derived_from_grep_cross_refs`

### P5 — Skill split into four sub-skills

- Author the four sub-skill files at
  `/Users/gdc/deadreckon/skills/<name>/SKILL.md` per the
  prescriptions in §"Sub-skill prompt prescriptions".
- Each file has the YAML frontmatter shape from predecessor
  (`name`, `description`, `output: json`).
- `run-narrator/SKILL.md` (legacy) is preserved unmodified.

Depth tests:
- `four_subskill_files_present_in_repo_skills_dir`
- `each_subskill_has_required_frontmatter_fields`
- `narrator_overview_prompt_asks_for_reading_order_and_why_now`
- `narrator_phases_prompt_requires_per_phase_paragraph_and_diff_quote`
- `narrator_as_built_prompt_forbids_project_files_layer`
- `narrator_as_built_prompt_requires_load_bearing_and_seams`
- `narrator_decisions_prompt_filters_false_positive_candidates`
- `legacy_run_narrator_skill_still_present`

### P6 — Polish orchestration: 4 sub-calls + merge

- New module `crates/deadreckon-core/src/polish_subcalls.rs`:
  `resolve_subskills(paths) -> Vec<SubSkill>`,
  `polish_subcalls(subskills, provider, inputs) -> Result<MergedDocs>`.
- Per sub-call: 16 K output token budget. Run sequentially (not
  parallel) so a sub-call can fail without losing the others'
  output.
- Merge: `narrator-overview` provides intro/closing of
  `RUN-NARRATIVE.md`; `narrator-phases` provides the body;
  `narrator-as-built` is the full `RUN-AS-BUILT.md`;
  `narrator-decisions` is the full `RUN-DECISIONS.md`.
- `polish.json` records each sub-call result.
- A failed sub-call flips that doc's section to the templated
  draft; other sub-calls' output still lands.
- Idempotency hash is unchanged shape but covers the new inputs.

Depth tests:
- `polish_runs_four_subcalls_sequentially`
- `polish_subcall_failure_does_not_abort_other_subcalls`
- `polish_merges_overview_into_narrative_intro`
- `polish_merges_phases_into_narrative_body`
- `polish_records_per_subcall_status_in_polish_json`
- `polish_per_subcall_token_budget_is_16k`
- `polish_idempotent_when_inputs_unchanged_across_subcalls`
- `polish_total_cost_summed_across_subcalls`

### P7 — New placeholders + bigger inputs

- Implement substitution for `{{ diff_samples }}` (compact rendering
  of per-file `+adds/-dels` + largest-hunk excerpts),
  `{{ tool_stdout }}` (concatenated bash stdout/stderr per turn),
  `{{ source_layout }}` (path-derived component-table seed),
  `{{ parent_narrative }}` (parent's `RUN-NARRATIVE.md` for extends).
- Per-skill placeholder set: each sub-skill receives the placeholders
  it actually uses (declared in its frontmatter via a new `inputs:`
  list); unknown placeholders pass through unchanged.

Depth tests:
- `diff_samples_placeholder_renders_per_file_blocks`
- `tool_stdout_placeholder_omits_non_bash_turns`
- `source_layout_placeholder_uses_path_inference`
- `parent_narrative_placeholder_empty_for_solo_runs`
- `parent_narrative_placeholder_loaded_for_extend_runs`
- `unknown_placeholder_passes_through_unchanged`

### P8 — Diff-coverage retry adapted to multi-section

- Adapt the predecessor's diff-coverage retry to multi-section:
  if `narrator-phases` omits a file, only re-invoke
  `narrator-phases` (not all four sub-calls).
- Cap retries at 2 per sub-call (predecessor default).
- Coverage is checked against `narrator-phases` output only;
  other sub-skills are not coverage-gated.

Depth tests:
- `coverage_retry_targets_only_narrator_phases`
- `coverage_retry_capped_at_two_per_subcall`
- `coverage_warning_logged_when_still_missing_after_two_retries`
- `non_phases_subcalls_not_coverage_gated`

### P9 — `deadreckon doc --force` and `--budget-cap`

- `--force` (existing flag) is wired to bypass `inputs_hash`
  short-circuit and re-run all four sub-calls.
- `--budget-cap <usd>` (new) refuses to polish above the cap with a
  `try:` hint; cost is estimated by per-skill output budget × model
  rate; CLI subscription providers are always considered $0.

Depth tests:
- `doc_polish_force_ignores_inputs_hash`
- `doc_polish_budget_cap_refuses_above_threshold`
- `doc_polish_budget_cap_zero_for_subscription_providers`
- `doc_polish_force_re_runs_all_subcalls_not_just_changed`

### P10 — Cross-cutting friendliness pass

- Polish preview block before each call:
  ```
  Polish preview:
    Provider: cli:codex (auto)
    Sub-skills: narrator-overview, narrator-phases, narrator-as-built, narrator-decisions
    Estimated cost: $0.00 (subscription)
    Inputs hash: 8a1f...
  Continue? [Y/n]
  ```
- Honor `--no-confirm` (existing).
- Honor `DEADRECKON_HINTS=0` (from audit-harden rider) on the
  preview block.
- Post-polish summary: 1-line per sub-call (`ok`, `failed: <reason>`,
  `skipped: <reason>`).
- Update `--help` for `deadreckon doc` to describe the new flags.

Depth tests:
- `polish_preview_block_lists_provider_and_subskills`
- `polish_preview_skipped_with_no_confirm`
- `polish_preview_suppressed_by_hints_env`
- `post_polish_summary_lists_each_subcall_status`
- `doc_help_describes_force_and_budget_cap_flags`

### P11 — AS-BUILT update + CHANGELOG (doc only; no depth test)

- Update `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` §25
  ("Self-Documenting Runs"):
  - Add §25.11 "Skill split into four sub-skills".
  - Add §25.12 "Per-turn capture richness (response, diff, stdio)".
  - Add §25.13 "Doc-provider auto-resolution".
  - Add §25.14 "Component-table inference and topology generation".
  - Add §25.15 "Polish preview and budget cap".
- Update §22 ("Built vs Scaffolding-Thin"):
  - Move `Self-documenting docs in stoa shape` from "Built and
    reliable" → keep listed; add a sub-bullet noting the doc-depth
    upgrade landed.
  - Note explicitly that this rider does not close any other §22
    thin items; it deepens an already-built capability.
- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:

  ```
  ## Doc depth (alpha) — 2026-05-11

  - Per-turn capture extended: full provider response (50 KB cap), per-file diff samples with largest-hunk excerpts, bash stdout/stderr (10 KB cap each).
  - run-narrator skill split into four sub-skills (narrator-overview, narrator-phases, narrator-as-built, narrator-decisions); each gets 16 K output tokens; same three-tier resolution.
  - doc_provider auto-resolves to in-PATH subscription CLI provider (cli:codex / cli:claude-code) when none is configured; previous "Doc-writer: templated only" path is now reached only when no provider is usable.
  - Templated narrative no longer truncates the title at 40 chars; per-turn outcomes no longer cut at 200 chars; phase prose synthesizes per-turn summaries instead of "deadreckon progressed through turn N".
  - Component-table inference uses path rules (crates/, skills/, docs/, etc.); generic "Project files" row is never emitted.
  - Process topology ASCII is generated only when ≥ 3 top-level directories changed.
  - deadreckon doc gains --force (bypass inputs hash) and --budget-cap <usd> flags.
  - polish.json schema bumped to 2 (per-sub-call records); v1 polish files read silently.
  ```

## Integration matrix

| Surface | What changes |
|---|---|
| `deadreckon run` | Per-turn capture richness; polish auto-runs when subscription provider in PATH |
| `deadreckon doc <id>` | `--force` and `--budget-cap` flags; preview block; per-sub-call summary |
| `deadreckon config` | New keys: `defaults.doc_provider`, `defaults.doc_subskills` (existing flag form remains) |
| Frontmatter | `Doc-writer:` line names the provider AND the sub-skills used |
| TUI `attach` | `polish.json` per-sub-call status surfaces in the docs view |
| `deadreckon list` | DOCS column unchanged (`polished` / `incremental` / `failed` / `n/a`) |
| `deadreckon library` | (audit-harden rider) — search across the now-richer narratives |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| `no doc provider available` | `deadreckon config set defaults.doc_provider cli:codex` |
| `polish sub-skill '<name>' returned malformed JSON twice` | `deadreckon doc <id> --polish --force --doc-skill <name>` |
| `polish would cost ~$<X>; cap is $<Y>` | `deadreckon doc <id> --polish --budget-cap <X>` or `--no-confirm` |
| `all polish sub-skills failed; templated drafts survive` | `deadreckon doc <id> --polish --force` |
| `narrator-phases coverage missing files: <list>` | `deadreckon doc <id> --polish --force` (informational; non-fatal) |
| `unknown sub-skill '<name>' in defaults.doc_subskills` | `deadreckon config set defaults.doc_subskills narrator-overview,narrator-phases,narrator-as-built,narrator-decisions` |

(Each parameterized over a depth test; see P2/P6/P8/P9.)

## Config additions (`config.toml`)

```toml
[defaults]
# Existing keys (provider, sandbox, max_spend, doc_skill, doc_provider, ...) unchanged.
doc_subskills = "narrator-overview,narrator-phases,narrator-as-built,narrator-decisions"
doc_polish_token_budget = 16384      # per sub-call output tokens
doc_polish_budget_cap_usd = 5.0      # global default cap; --budget-cap overrides per call
```

Absence of these keys → defaults baked in the binary.

## Out of scope (explicitly not in this milestone)

- **Streaming polish output to TUI.** The TUI shows the final docs;
  per-sub-call streaming is V1.
- **Cross-run rollups** ("what shipped across all runs in 7 days").
  Library `search` covers the inspect slice (audit-harden rider).
  Aggregate narratives are V1.
- **HTML / PDF export.** `doc --export` writes markdown only.
- **Parallel sub-call execution.** Sequential is correct for alpha;
  parallel is a V1 optimization once the cost model is clear.
- **Per-sub-skill providers.** All four sub-calls use the same
  `doc_provider`. Mixing providers per sub-skill is V1.
- **Inline image / chart rendering.** ASCII only; mermaid/graphviz
  is V1.
- **Doc lints** ("your phase is missing a why-now sentence"). The
  prompts nudge but don't gate.
- **Translation / multi-language docs.** English only.
- **Auto-PR with the docs as PR body.** `apply` already builds the
  body from `RUN-NARRATIVE.md`; no remote calls.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (utility, free):

- No new direct deps. `regex` is already approved (predecessor).
  `serde_json` covers the schema. Diff parsing uses `git` shelled
  out (already in PATH for codebase-mode runs); for non-git dirs
  we call `similar` if and only if it's already in the workspace
  tree — otherwise fall back to whole-file capture truncated to the
  hunk-excerpt cap (no new dep).

Tier 2 (architectural, log to `DEPENDENCIES.md`): none expected.

Tier 3 (blocked): same blocks as prior riders.

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes.** All new state in files.
- **One depth test before each phase implementation.** A phase
  whose tests were never red is suspect.
- **Backwards-compatible.** Single-skill installs keep working;
  v1 `polish.json` upgrades silently; legacy `run-narrator` skill
  is preserved.
- **The four sub-skill prompts are the spec for substance.** The
  text is free to evolve; the **required output sections** named
  in §"Sub-skill prompt prescriptions" are pinned by depth tests.
- **`Project files` is forbidden** as a components-table row.
  Depth-tested. The rule is: better to omit a row than emit a
  meaningless one.
- **No silent expansion.** Anything beyond P1–P11 goes into
  `V1-CANDIDATES.md`.
- **Spec-pinning invariants.** Frontmatter format (predecessor),
  `_incremental.jsonl` extended schema, `polish.json` v2 shape,
  topology ASCII glyphs, error-footer text are all depth-tested;
  changing whitespace or ordering changes the spec.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with the relevant depth tests passing and a
  CHANGELOG entry naming the SHA.
- After P11, run a smoke flow end-to-end: re-polish the existing
  ms-paint run at
  `/Users/gdc/test-deadreckon/build-a-full-ms-paint-ty/` via
  `deadreckon doc <id> --polish --force` (read-only on that path
  outside `/Users/gdc/deadreckon/` is fine; the verb only writes
  to the library entry under `~/.deadreckon/library/`); compare
  the new `RUN-NARRATIVE.md` length to the 60-line baseline.
- If a phase reveals a V1-architecture decision, stop and log it in
  `V1-CANDIDATES.md`; do not silently expand scope.
