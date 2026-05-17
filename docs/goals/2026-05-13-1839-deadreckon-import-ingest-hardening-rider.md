# deadreckon - Import Ingest Hardening Rider

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-13-1839-deadreckon-import-ingest-hardening-goal.md`.
It supersedes nothing in prior riders, especially provider CLI ingest,
Copilot/Pi providers, robustness import normalization, and audit hardening.
Their invariants still apply. This rider turns `deadreckon import` into a
session-aware, descriptor-backed, read-only recovery/import workflow.

**All paths absolute.** Source `/Users/gdc/deadreckon/`; runtime
`/Users/gdc/.deadreckon/`; provider-owned transcript roots are read-only
inputs.

## Posture (decided - do not redesign)

- **Maturity stays `alpha`.**
- **No `PipelineState` schema changes.** Import metadata lives in sidecar files
  under the imported run root.
- **Provider-owned transcripts are read-only.** Do not truncate, rewrite,
  migrate, delete, or mark source files under `~/.codex`, `~/.claude`,
  `~/.gemini`, `~/.local/share/opencode`, `~/.copilot`, `~/.pi`, or Cursor.
- **Provider `[ingest]` is the source of truth for CLI transcript discovery.**
  The import command may keep Cursor as a legacy non-provider source, but should
  not add a second CLI-provider root table.
- **No OpenCode SQLite unless dependency-free and tiny.** File-mode OpenCode is
  in scope because the TUI ingest path already supports it.
- **No live provider calls in tests.** Fixtures and fake homes only.
- **No `git push`.** Phased local commits only.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Current Assessment

Original intent, from the build/robustness/audit riders: `deadreckon import`
should provide read-only cross-tool state sharing. External coding-agent
history should become deadreckon run state so `show`, provenance inspection,
library search, and later handoff/recovery tools can reason about work that
happened outside deadreckon.

Current implementation in `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs`
around `import_command` and `normalize_import` is useful but first-pass:

- accepted sources are hard-coded strings: `claude-code`, `codex`, `cursor`;
- default roots are hard-coded and separate from provider descriptors;
- JSONL import recursively scans the whole root and imports every row;
- run id is deterministic from `source:root`, so a root-level reimport silently
  replaces the previous imported run;
- trace details mostly preserve source rows plus `source_path` / `source_line`;
- provenance extraction only sees top-level `path`, `file`, and `files`;
- malformed JSONL errors are actionable, but empty, ambiguous, stale, and
  "wrong cwd" cases are not first-class UX states;
- import does not benefit from today's descriptor `[ingest]` roots, cwd
  matching, storage kinds, schema dispatch, or newer provider schemas.

This rider does **not** criticize the original import. It was intentionally an
inventory-level bridge. The next step is to make import session-aware and to
reuse the provider-ingest substrate already built for attach.

## User Surface

Keep the existing command shape, extending it with flags:

```text
deadreckon import <source>
deadreckon import <source> --preview
deadreckon import <source> --list
deadreckon import <source> --session <id-or-path>
deadreckon import <source> --cwd <path>
deadreckon import <source> --all
deadreckon import <source> --since <duration>
deadreckon import <source> --replace
deadreckon import <source> --json
```

Accepted sources:

| User input | Resolution |
|---|---|
| `codex` | `cli:codex` descriptor ingest |
| `claude-code` | `cli:claude-code` descriptor ingest |
| `gemini` / `cli:gemini` | `cli:gemini` descriptor ingest |
| `opencode` / `cli:opencode` | `cli:opencode` descriptor ingest |
| `copilot` / `cli:copilot` | `cli:copilot` descriptor ingest |
| `pi` / `cli:pi` | `cli:pi` descriptor ingest |
| `cursor` | legacy Cursor SQLite import adapter |

Default behavior:

- Resolve the source.
- Discover candidate sessions using descriptor ingest metadata or Cursor roots.
- Filter to the current cwd unless `--cwd` or `--all` changes the target.
- If exactly one viable session remains, preview then import it. In a TTY, the
  preview can ask for confirmation using existing local patterns; in
  non-interactive mode it should require `--yes` only if the repo already has
  that convention for comparable state creation. If no such convention exists,
  `--preview` is explicit no-write and normal import writes after printing a
  concise preflight.
- If zero or multiple sessions remain, refuse without creating a run and print
  candidate rows plus `try:` lines.
- `--all` preserves the old whole-root intent, but it is explicit and records
  every source file in the manifest.

Refusal examples:

| Case | Required behavior |
|---|---|
| unknown source | list accepted sources and `try: deadreckon import codex --list` |
| descriptor has no `[ingest]` | `try: deadreckon providers list --all` and explain no importable transcript root |
| no candidates | show resolved roots and `try: deadreckon import <source> --all --preview` |
| ambiguous candidates | print session ids/paths and `try: deadreckon import <source> --session <id>` |
| existing import run with changed content | refuse unless `--replace`; show old/new hash |
| malformed source row | include file/line/path and keep current actionable JSON error style |

## Data Model (files, not fields)

Write `/Users/gdc/.deadreckon/runstate/<scope>/runs/<run-id>/import.json`:

```json
{
  "version": 1,
  "source": "cli:codex",
  "source_alias": "codex",
  "schema": "codex-cli",
  "storage": "jsonl",
  "cwd": "/absolute/workspace",
  "mode": "session",
  "session_id": "optional-provider-session-id",
  "session_paths": ["/absolute/source/session.jsonl"],
  "content_hash": "sha256:...",
  "imported_at": "2026-05-13T18:39:00Z",
  "source_started_at": "optional-rfc3339",
  "source_updated_at": "optional-rfc3339",
  "rows_seen": 42,
  "events_imported": 17,
  "provenance_records": 5,
  "raw_rows_stored": false,
  "reimport_command": "deadreckon import codex --session ... --replace"
}
```

Trace `detail` shape should be stable and source-neutral:

```json
{
  "import_version": 1,
  "source": "cli:codex",
  "schema": "codex-cli",
  "session_id": "abc",
  "source_path": "/absolute/source/session.jsonl",
  "source_line": 12,
  "source_event": "tool_call",
  "role": "assistant",
  "summary": "tool Bash cargo test",
  "tool_name": "exec_command",
  "tool_category": "Bash",
  "tool_call_id": "call-123",
  "files": ["src/main.rs"],
  "usage": { "input_tokens": 100, "output_tokens": 25, "context_window": 258400 },
  "raw_hash": "sha256:..."
}
```

Do not store huge raw provider payloads in every trace row by default. Preserve
source path/line and raw hashes so the source can be audited when still
available. If the executor chooses to store a raw snapshot, put it under an
import-owned path such as `run_root/import/raw/` and record that in
`import.json`; do not copy entire home roots.

## Architecture

Add a small import/discovery layer instead of growing `main.rs` further.
Recommended files:

- `/Users/gdc/deadreckon/crates/deadreckon/src/import.rs`
- `/Users/gdc/deadreckon/crates/deadreckon/src/provider_logs.rs`

`provider_logs.rs` should own descriptor-backed candidate discovery shared by
attach and import:

```rust
struct ProviderLogSpec {
    source: String,
    schema: String,
    roots: Vec<PathBuf>,
    since: DateTime<Utc>,
    cwd_match: IngestCwdMatch,
    cwd_match_path: Option<String>,
    storage: IngestStorage,
    file_glob: Option<String>,
}

struct ProviderLogCandidate {
    session_id: Option<String>,
    paths: Vec<PathBuf>,
    updated_at: DateTime<Utc>,
    matched_cwd: Option<PathBuf>,
    row_count_hint: Option<usize>,
}
```

Import parsing can live beside existing activity parsing at first. Do not
attempt a grand parser framework. Add schema-specific functions that emit
normalized import events, and only factor shared helpers once at least two
schemas need them.

```rust
struct ImportedEvent {
    timestamp: Option<DateTime<Utc>>,
    source_event: String,
    role: Option<String>,
    summary: String,
    tool_name: Option<String>,
    tool_category: Option<String>,
    tool_call_id: Option<String>,
    files: Vec<PathBuf>,
    usage: Option<ImportedUsage>,
    source_path: PathBuf,
    source_line: Option<usize>,
    raw_hash: String,
}
```

The attach TUI should keep emitting the same activity lines. Any extraction
must preserve current provider activity tests.

## Source-Specific Expectations

### Codex

- Discover via `cli:codex` `[ingest]`.
- Match cwd through `session_meta.payload.cwd`.
- Import agent messages, function calls, function outputs, token counts, and
  file-affecting tool arguments.
- Preserve current `codex.show.golden` behavior while replacing raw row details
  with normalized details in new goldens.

Depth fixtures must include a function call with arguments that name a path and
a token-count event.

### Claude Code

- Discover via `cli:claude-code` `[ingest]`.
- Keep Claude project-dir root transformation.
- Import assistant text, thinking blocks, `tool_use`, `tool_result`, and usage.
- Extract file paths from common tool names/inputs, including edit/write/read
  shapes when present.

### Gemini

- Discover via `cli:gemini` `[ingest]` and support JSON or JSONL file shape.
- Because cwd matching is `none`, default import should require `--session` or
  a single fresh candidate unless the candidate list is unambiguous.
- Import text, thinking, tool calls, and tool results from the existing TUI
  parser fixture shape.

### OpenCode

- File-mode storage only: `storage/session`, `storage/message`, `storage/part`.
- Import session metadata, assistant messages, tool parts, result parts, and
  file paths.
- SQLite-backed OpenCode remains in `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`
  unless the executor proves it can be read without new dependency/build risk.

### Copilot

- Discover via `cli:copilot` `[ingest]`, including bare `session-state/*.jsonl`
  and nested `events.jsonl`.
- Import `assistant.message`, `assistant.reasoning`, `tool.execution_complete`,
  `session.model_change`, and usage in camelCase/snake_case variants.

### Pi

- Discover via `cli:pi` `[ingest]`.
- Validate the session header when available.
- Import assistant text/thinking/toolCall blocks, tool results, model changes,
  and usage.

### Cursor

- Keep `DEADRECKON_IMPORT_CURSOR_ROOT`.
- Continue using `sqlite3 -json` if no Rust SQLite dependency exists.
- Record Cursor as `source = "cursor"` and `schema = "cursor-sqlite"` in
  `import.json`.
- Preserve existing Cursor golden coverage.

## Phases

Each phase: write the named depth tests first and watch them fail; implement;
run focused verification; conventional local commit; one-line CHANGELOG entry.

### P1 - Import UX contract and source resolution

- Extend clap flags and help.
- Resolve aliases to descriptor IDs or Cursor.
- Add preview/list no-write paths.

Depth tests in `/Users/gdc/deadreckon/crates/deadreckon/tests/agentic_loop.rs`:

- `import_accepts_descriptor_provider_ids_and_legacy_aliases`
- `import_preview_does_not_create_run`
- `import_unknown_source_lists_supported_sources_with_try_line`

### P2 - Shared descriptor-backed candidate discovery

- Extract provider ingest spec/candidate discovery so attach and import share
  roots, env overrides, file glob, freshness, storage, and cwd matching.
- Keep attach behavior compatible.

Depth tests:

- `import_discovery_uses_descriptor_roots_and_env_override`
- `import_discovery_filters_candidates_by_cwd`
- `provider_activity_still_uses_descriptor_ingest_after_extraction`

### P3 - Import manifest and deterministic session IDs

- Write `import.json`.
- Run IDs should be deterministic for a concrete source session, not only
  `source:root`.
- Existing changed-content imports refuse unless `--replace`.

Depth tests:

- `import_writes_manifest_with_source_schema_hash_and_reimport_command`
- `import_run_id_is_stable_for_same_session_not_entire_root`
- `reimport_changed_session_requires_replace`

### P4 - Normalized event model

- Add `ImportedEvent` / usage helpers.
- Write stable normalized trace details.
- Use source timestamps when available; fallback to import time.

Depth tests:

- `import_trace_detail_uses_stable_normalized_schema`
- `import_uses_source_timestamps_when_present`
- `import_raw_large_payloads_are_not_duplicated_in_every_trace`

### P5 - Codex and Claude import parsers

- Implement normalized import extraction for existing first-class JSONL
  sources.
- Preserve source path/line and current golden intent.

Depth tests:

- `import_codex_extracts_tool_calls_tokens_and_file_provenance`
- `import_claude_extracts_content_blocks_usage_and_file_provenance`
- `import_codex_and_claude_fixture_goldens_use_normalized_details`

### P6 - Gemini and OpenCode file-mode import parsers

- Add fixtures matching the current TUI ingest parser shapes.
- Keep Gemini ambiguous-cwd behavior explicit.
- Keep OpenCode file mode only.

Depth tests:

- `import_gemini_requires_session_when_cwd_match_is_none_and_ambiguous`
- `import_gemini_fixture_round_trips_to_show`
- `import_opencode_file_mode_fixture_round_trips_to_show`

### P7 - Copilot and Pi import parsers

- Reuse the provider schemas added by the Copilot/Pi goal.
- Cover usage and tool/result extraction.

Depth tests:

- `import_copilot_fixture_round_trips_to_show`
- `import_copilot_nested_events_file_is_discovered`
- `import_pi_fixture_round_trips_to_show`

### P8 - Cursor compatibility

- Move Cursor into the import module without changing user behavior.
- Preserve existing SQLite tests and goldens.
- Add manifest coverage for Cursor.

Depth tests:

- `import_cursor_fixture_round_trips_to_golden`
- `import_cursor_writes_manifest_with_sqlite_source`
- `import_cursor_sqlite_error_keeps_actionable_message`

### P9 - Friendly refusals and lifecycle hints

- Normalize all import errors through a formatter with `try:` lines.
- Empty roots, stale sessions, ambiguous matches, and no `[ingest]` become
  tested first-class cases.
- Completion output should hint `show`, `attach`, and `cleanup`/reimport.

Depth tests:

- `import_empty_root_refuses_with_resolved_roots_and_try_line`
- `import_ambiguous_sessions_prints_candidate_table_and_try_line`
- `import_completion_prints_show_and_reimport_hints`

### P10 - Golden refresh and focused compatibility matrix

- Refresh import fixtures/goldens only after normalized schema is stable.
- Ensure attach provider activity tests still pass.
- Ensure old `deadreckon import codex` and `deadreckon import claude-code`
  remain usable.

Depth tests:

- `import_legacy_codex_command_remains_supported`
- `import_legacy_claude_code_command_remains_supported`
- `provider_jsonl_activity_dispatches_all_import_supported_provider_schemas`

### P11 - Architecture doc, CHANGELOG, and V1 accounting

- Update `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:
  - Section 16 Cross-Tool Import: descriptor-backed sources, session selection,
    manifest, normalized traces/provenance.
  - Section 18 TUI: shared provider log discovery remains behavior-compatible.
  - Section 22 Built vs Scaffolding-Thin: mark descriptor import hardening as built.
- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:

```markdown
## Descriptor import hardening (alpha) - 2026-05-13

- Reworked `deadreckon import` around descriptor-backed provider transcript
  discovery, concrete session selection, import manifests, and normalized
  trace/provenance events while preserving Cursor SQLite import.
```

- Update `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` for deferred:
  OpenCode SQLite, richer replay, provider transcript mutation/undo, and
  cross-run import analytics.

## Focused Verification Ladder

Use targeted checks during phases:

```zsh
cargo test -p deadreckon --test agentic_loop import_<filter>
cargo test -p deadreckon provider_jsonl
cargo test -p deadreckon-providers --test registry ingest
cargo fmt --check
cargo clippy -p deadreckon --tests -- -D warnings
cargo clippy -p deadreckon-providers --tests -- -D warnings
```

Final focused matrix:

```zsh
cargo test -p deadreckon --test agentic_loop import
cargo test -p deadreckon provider_jsonl
cargo test -p deadreckon-providers --test registry
cargo fmt --check
cargo clippy -p deadreckon --tests -- -D warnings
cargo clippy -p deadreckon-providers --tests -- -D warnings
```

Run the full workspace suite only if requested or after the focused matrix is
green and budget permits.

## Error-Footer Canonical Pairs

| Error | `try:` |
|---|---|
| unknown source | `deadreckon import codex --list` |
| no importable descriptor ingest | `deadreckon providers list --all` |
| no candidates | `deadreckon import <source> --all --preview` |
| ambiguous candidates | `deadreckon import <source> --session <id-or-path>` |
| changed content for existing import | `deadreckon import <source> --session <id-or-path> --replace` |
| malformed JSONL | `fix or exclude <path>; then rerun deadreckon import <source> --session <id-or-path>` |
| Cursor sqlite3 unavailable | `install sqlite3 or pass a JSONL-capable provider source` |

## Out of scope

- Mutating provider-owned transcripts.
- Provider transcript undo/truncation.
- New `PipelineState` fields.
- Full replay UI or analytics across imported sessions.
- OpenCode SQLite unless dependency-free and tiny.
- Importing hosted/cloud-only products with no local transcript files.
- Live provider calls during import.

## Dependencies

Prefer existing dependencies: `serde`, `serde_json`, `chrono`, `sha2` if already
present, `tempfile`, `assert_cmd`, and `toml`. If a new hash/glob dependency is
needed, justify it in `DEPENDENCIES.md`; otherwise implement simple extension
matching like the current ingest collector.

Cursor may continue shelling out to `sqlite3`. Do not add a Rust SQLite
dependency just for Cursor in this rider.

## Engineering invariants

- No `PipelineState` schema changes.
- Provider-owned transcripts are read-only.
- Import discovery for CLI providers uses descriptor `[ingest]`.
- `--preview` and `--list` create no run directories.
- Default import never silently merges many unrelated sessions.
- Every refusal includes a `try:` line.
- Existing attach provider activity semantics remain compatible.
- Cursor remains supported.

## Process invariants

- Phased local commits only. No `git push`.
- Write depth tests before implementation in every P1-P10 phase.
- Refresh goldens only after the normalized trace schema is final for this
  rider.
- If parser richness grows beyond trace/provenance recovery, log it in
  `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` rather than expanding scope.
