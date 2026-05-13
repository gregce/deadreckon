# deadreckon - Provider CLI Ingest Rider (descriptor to TUI)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-13-1345-deadreckon-provider-cli-ingest-goal.md`.
It supersedes nothing in prior riders (especially
`2026-05-11-2248-deadreckon-provider-registry-rider.md` and
`2026-05-12-2039-deadreckon-hygiene-rider.md`) - their invariants still
apply. This rider adds descriptor-driven CLI log ingest, generic CLI
launch through descriptor `exec_template`, canonical tool categories, and
two pilot agent CLIs.

**All paths absolute.** Source `/Users/gdc/deadreckon/`; research
reference `/Users/gdc/agentsview/`; runtime `~/.deadreckon/`.

## Posture (decided - do not redesign)

- **Maturity stays `alpha`.**
- **No `PipelineState` schema changes.** This work may extend provider
  descriptors and tests, but not run-state schemas.
- **Provider-owned transcripts are read-only.** Do not truncate, rewrite,
  migrate, or "undo" files under `~/.codex`, `~/.claude`, `~/.gemini`, or
  `~/.local/share/opencode`.
- **No runtime dependency on `/Users/gdc/agentsview`.** Use its parser
  registry as research/spec. Port small ideas into Rust.
- **No SQLite dependency unless it is explicitly proven low-risk.**
  OpenCode file mode is in scope. SQLite mode is a V1 candidate by
  default.
- **No `git push`.** Phased local commits only.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Verification Ladder (focused, not full-suite)

This rider intentionally does **not** require `make verify` after every
phase. The affected surface is provider descriptors, CLI routing, and TUI
ingest, so the default loop is targeted verification only.

Do not run these by default during P1-P10:

- `make verify`
- `cargo build --release`
- `cargo test --workspace`
- `make smoke`
- `make stress`

Per phase, run only the named depth tests plus the smallest relevant crate
checks:

```zsh
cargo test -p deadreckon-providers --test registry <filter>
cargo test -p deadreckon-providers --test cli_providers <filter>
cargo test -p deadreckon --test providers_list <filter>
cargo test -p deadreckon provider_jsonl
cargo test -p deadreckon detect
cargo fmt --check
cargo clippy -p deadreckon-providers --tests -- -D warnings
cargo clippy -p deadreckon --tests -- -D warnings
```

Choose the subset that matches touched files:

- Descriptor/schema/taxonomy changes: provider crate tests + provider clippy.
- Router/generic CLI changes: `cli_providers` + provider clippy.
- `main.rs` TUI/detect/init changes: filtered `deadreckon` tests + binary
  crate clippy.
- Doc-only P10/P11 changes: doc assertion tests plus `cargo fmt --check` only
  if Rust files were touched.

At the end, run the focused acceptance matrix, not the whole workspace:

```zsh
cargo test -p deadreckon-providers --test registry
cargo test -p deadreckon-providers --test cli_providers
cargo test -p deadreckon --test providers_list
cargo test -p deadreckon provider_jsonl
cargo test -p deadreckon detect
cargo fmt --check
cargo clippy -p deadreckon-providers --tests -- -D warnings
cargo clippy -p deadreckon --tests -- -D warnings
```

Only run `make verify` or release builds if the user explicitly asks for the
full suite or the final executor has budget and no known long-running blocker.
If skipped, say so in the final handoff with the focused commands that did run.

## Data Model

Extend `/Users/gdc/deadreckon/crates/deadreckon-providers/src/registry/mod.rs`
with optional ingest metadata. The field names below are the spec; adapt
types conservatively to local Rust style.

```rust
pub struct ProviderDescriptor {
    // existing fields...
    pub ingest: Option<IngestDescriptor>,
}

#[serde(rename_all = "kebab-case")]
pub enum IngestCwdMatch {
    None,
    SessionMeta,
    TopLevel,
    JsonPointer,
    ClaudeProjectDir,
    DirectoryField,
}

pub struct IngestDescriptor {
    pub id_prefix: Option<String>,
    pub env_var: Option<String>,
    pub default_dirs: Vec<PathBuf>,
    pub watch_subdirs: Vec<PathBuf>,
    pub shallow_watch: bool,
    pub schema: String,
    pub cwd_match: IngestCwdMatch,
    pub cwd_match_path: Option<String>,
    pub session_id_from: Option<String>,
    pub file_glob: Option<String>,
    pub freshness_minutes: Option<i64>,
    pub storage: Option<IngestStorage>,
}

#[serde(rename_all = "kebab-case")]
pub enum IngestStorage {
    Jsonl,
    Json,
    JsonOrJsonl,
    OpenCodeStorage,
}
```

Descriptor examples to backfill:

```toml
[ingest]
id_prefix = "codex:"
env_var = "CODEX_SESSIONS_DIR"
default_dirs = ["~/.codex/sessions", "~/.codex/archived_sessions"]
schema = "codex-cli"
cwd_match = "session-meta"
cwd_match_path = "payload.cwd"
file_glob = "*.jsonl"
freshness_minutes = 2
storage = "jsonl"
```

```toml
[ingest]
env_var = "CLAUDE_PROJECTS_DIR"
default_dirs = ["~/.claude/projects"]
schema = "claude-code"
cwd_match = "claude-project-dir"
file_glob = "*.jsonl"
freshness_minutes = 2
storage = "jsonl"
```

## Algorithms

### Descriptor roots

Resolution order:

1. If `ingest.env_var` is set and the environment variable is non-empty,
   split it by the platform path separator and use those roots.
2. Otherwise expand `ingest.default_dirs` relative to `$HOME`.
3. Apply provider-specific root transforms only when declared by
   `cwd_match = "claude-project-dir"`: turn each run working dir into the
   Claude project slug, exactly matching existing
   `claude_project_dir_for_workdir`.
4. Deduplicate paths after expansion.

### Candidate discovery

- Keep the existing recency gate: `started_at - freshness_minutes`, default
  `2`.
- Existing JSONL recursive collection becomes generic over file extensions
  and storage shape.
- Support `jsonl`, `json`, and `json-or-jsonl` without new dependencies.
- OpenCode file mode uses:
  `~/.local/share/opencode/storage/session/<project>/*.json` and reads
  sibling `storage/message` and `storage/part` files as needed.
- Cap TUI activity at the same 240 rows unless a test proves the cap needs to
  move.

### Cwd matching

Supported strategies:

- `session-meta`: scan the first 8 JSONL lines for `type=session_meta` and
  compare `payload.cwd`.
- `top-level`: scan the first 80 JSONL lines for top-level `cwd`.
- `json-pointer`: scan a bounded prefix and compare `cwd_match_path`.
- `claude-project-dir`: roots are already cwd-scoped; still accept top-level
  `cwd` when present.
- `directory-field`: read JSON field `directory` and compare run working dirs.
- `none`: accept the freshest candidate for the active provider only. Use this
  sparingly; Gemini needs it because the transcript does not reliably carry
  cwd.

### Parser dispatch

Do not make row parsing declarative. Use a Rust dispatch table:

```rust
struct IngestParser {
    schema: &'static str,
    parse_line: fn(&str, &mut ProviderActivity) -> Vec<String>,
    parse_file: Option<fn(&Path, &mut ProviderActivity) -> Result<()>>,
}
```

Codex and Claude can stay line-based. Gemini may parse file-shaped JSON or
JSONL. OpenCode file mode should use a file parser.

## Provider Descriptors In Scope

### `cli:gemini`

Research: `/Users/gdc/agentsview/internal/parser/types.go` Gemini registry
entry and `/Users/gdc/agentsview/internal/parser/gemini.go`.

Descriptor shape:

- `id = "cli:gemini"`
- `kind = "cli"`
- `default_binary = "gemini"`
- `sandbox_writes = ["~/.gemini"]`
- `sandbox_reads = ["~/.gemini"]`
- `fs_detection_paths = ["~/.gemini"]`
- `[ingest] default_dirs = ["~/.gemini"]`, `watch_subdirs = ["tmp"]`,
  `schema = "gemini"`, `storage = "json-or-jsonl"`, `cwd_match = "none"`.

Launch command must be verified from the local binary or official docs before
shipping. If uncertain, land detect/ingest and put launch details in
`docs/V1-CANDIDATES.md`; do not invent arguments.

### `cli:opencode`

Research: `/Users/gdc/agentsview/internal/parser/types.go`,
`/Users/gdc/agentsview/internal/parser/discovery.go`, and
`/Users/gdc/agentsview/internal/parser/opencode.go`.

Descriptor shape:

- `id = "cli:opencode"`
- `kind = "cli"`
- `default_binary = "opencode"`
- `sandbox_writes = ["~/.local/share/opencode"]`
- `sandbox_reads = ["~/.local/share/opencode"]`
- `fs_detection_paths = ["~/.local/share/opencode"]`
- `[ingest] default_dirs = ["~/.local/share/opencode"]`,
  `watch_subdirs = ["storage/session", "storage/message", "storage/part"]`,
  `schema = "opencode"`, `storage = "opencode-storage"`,
  `cwd_match = "directory-field"`.

SQLite-backed `opencode.db` is out of scope unless it can be read without
adding a new dependency or build risk. If deferred, document the exact virtual
path shape in `docs/V1-CANDIDATES.md`.

## Phases

Each phase: write the named depth test first and watch it fail; implement;
green on the focused verification ladder above; conventional-commit local
commit; one-line CHANGELOG entry.

### P1 - Descriptor ingest schema

- Add `IngestDescriptor`, `IngestCwdMatch`, and `IngestStorage`.
- Backfill `[ingest]` into Codex and Claude descriptors.
- Keep serde defaults backward-compatible: descriptors without `[ingest]`
  still load.

Depth tests in `/Users/gdc/deadreckon/crates/deadreckon-providers/tests/registry.rs`:

- `descriptor_ingest_round_trips_for_codex_and_claude`
- `descriptor_without_ingest_still_loads`
- `provider_override_can_replace_ingest_roots_without_losing_exec_template`

### P2 - Descriptor-driven TUI log spec

- Replace `provider_jsonl_log_spec`'s provider-ID match with a registry lookup.
- Root expansion uses `[ingest]`, `$HOME`, env override, and Claude project-dir
  transform.
- Keep existing behavior for Codex/Claude.

Depth tests in `/Users/gdc/deadreckon/crates/deadreckon/tests` or inline TUI
tests:

- `provider_log_spec_uses_descriptor_roots_for_codex`
- `provider_log_spec_honors_ingest_env_override`
- `claude_ingest_roots_remain_workdir_scoped_and_deduped`

### P3 - Schema dispatch table

- Replace `ProviderJsonlSchema` enum with a schema string resolved to parser
  functions.
- Unknown or missing schema returns no provider activity, not a panic.
- Existing Codex and Claude row parsers keep their output semantics.

Depth tests:

- `schema_dispatch_routes_codex_and_claude_rows`
- `schema_dispatch_unknown_schema_is_quiet`
- `existing_provider_jsonl_activity_dispatches_codex_and_claude_rows`
  updated, not deleted.

### P4 - Generic candidate discovery and cwd matching

- Generalize `collect_recent_jsonl_files` to extension/storage-aware discovery.
- Implement `session-meta`, `top-level`, `json-pointer`, `claude-project-dir`,
  `directory-field`, and `none`.
- Keep bounded scans: 8 lines for Codex, 80 lines for Claude/top-level.

Depth tests:

- `cwd_match_session_meta_reads_payload_cwd`
- `cwd_match_top_level_scans_bounded_prefix`
- `cwd_match_directory_field_matches_opencode_json`
- `freshness_gate_ignores_stale_provider_logs`

### P5 - Tool taxonomy

- Port agentsview `NormalizeToolCategory` into Rust as
  `deadreckon_providers::taxonomy::normalize_tool_category`.
- Use categories in TUI tool lines while preserving useful raw summaries.
  Recommended display: `tool Bash <summary>` rather than raw `exec_command`
  when the category is known.
- Keep raw tool names in summaries or traces where needed for debugging.

Depth tests in `/Users/gdc/deadreckon/crates/deadreckon-providers`:

- `taxonomy_matches_agentsview_core_cases`
- `taxonomy_subagent_names_map_to_task`
- `codex_tool_lines_use_normalized_categories`
- `claude_tool_lines_use_normalized_categories`

### P6 - Generic CLI provider from `exec_template`

- Add one descriptor-backed CLI provider that renders
  `exec_template.args_template`.
- Support placeholders:
  - `{prompt}`
  - `{sandbox}`
  - `{cwd}`
- Support descriptor `model_arg`; insert model flag only when configured model
  is not empty and not `"provider default"`.
- Preserve Codex's trailing `--` delimiter. Prefer representing it in
  `args_template`; if local behavior requires special handling, document why
  in code and tests.
- Router must build generic CLI providers for `ProviderKind::Generic(id)` when
  the descriptor kind is `cli`.

Depth tests in `/Users/gdc/deadreckon/crates/deadreckon-providers/tests/cli_providers.rs`:

- `generic_cli_provider_runs_descriptor_template`
- `generic_cli_provider_passes_model_arg_from_descriptor`
- `generic_cli_provider_preserves_codex_prompt_delimiter`
- `generic_cli_provider_uses_descriptor_sandbox_writes`

### P7 - Detection, init, and output-name generalization

- `init --yes` auto-selects the first credentialed subscription CLI from the
  registry, not only hard-coded Claude/Codex.
- `provider_output_name` derives a stable file name from descriptor id:
  `cli:gemini -> gemini.out`, `cli:opencode -> opencode.out`, fallback
  `provider.out`.
- `providers list` and `detect` include new descriptor-backed CLIs.

Depth tests:

- `init_yes_prefers_registry_cli_binary_order`
- `provider_output_name_slugifies_cli_descriptor_id`
- `detect_lists_new_cli_descriptors_with_install_hints`

### P8 - Gemini descriptor and ingest parser

- Add `cli-gemini.toml` if launch shape is verified; otherwise add a clearly
  detect/ingest-only descriptor only if the current descriptor model supports
  that honestly.
- Parser handles Gemini JSON object and JSONL shapes from agentsview:
  `sessionId`, `messages[]`, `type in {user, gemini}`, `thoughts`, `content`,
  `toolCalls[]`, inline function responses, and `tokens`.
- TUI lines should cover assistant text, thinking, tool calls, results, and
  context token telemetry.
- Fixtures must be minimal and synthetic; do not use private transcripts.

Depth tests:

- `gemini_json_object_fixture_emits_agent_tool_result_and_tokens`
- `gemini_jsonl_fixture_dedupes_repeated_message_ids`
- `gemini_ingest_none_cwd_uses_fresh_active_provider_candidate_only`

### P9 - OpenCode file-mode descriptor and ingest parser

- Add `cli-opencode.toml`.
- Implement file-mode discovery under
  `storage/session/<project>/<session>.json`; read corresponding
  `storage/message` and `storage/part` files only from the same OpenCode root.
- Parser handles user/assistant text, reasoning, tool parts, `directory`, and
  token fields from message data or `step-finish` parts.
- SQLite mode remains deferred unless explicitly accepted in P9 notes.

Depth tests:

- `opencode_storage_fixture_emits_agent_thinking_tool_and_tokens`
- `opencode_directory_field_matches_run_working_dir`
- `opencode_storage_ignores_sqlite_when_file_mode_present`

### P10 - Bulk-addition checklist and docs for remaining CLIs

- Add a small contributor-facing checklist in
  `/Users/gdc/deadreckon/docs/design/PROVIDER-CLI-INGEST.md`:
  descriptor fields, fixture shape, parser function, cwd match, sandbox writes,
  detect smoke, TUI smoke.
- Add a matrix row for Amp, iFlow, Copilot, OpenHands, Cortex, Hermes, Kiro,
  Pi/Kimi/OpenClaw/Zencoder with one of:
  `ready-for-descriptor`, `needs-launch-research`, `needs-parser-port`,
  `out-of-scope-non-cli`.
- Do not ship unverified launch commands.

Depth tests:

- `provider_ingest_design_lists_required_addition_checklist`
- `provider_ingest_design_marks_unverified_launch_commands_out_of_scope`

### P11 - Architecture doc and CHANGELOG

- Update `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:
  - §10 Provider Model: descriptor-backed generic CLI provider and remaining
    legacy compatibility, if any.
  - §18 TUI: descriptor-driven ingest and schemas now shipped.
  - §22 Built vs Thin: add provider CLI ingest as shipped; list deferred
    SQLite/undo/bulk-agent scope as V1.
- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:

```markdown
## Provider CLI ingest (alpha) - 2026-05-13

- ...
```

- Update `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` with any deferred
  decisions: OpenCode SQLite, provider-aware undo offsets, bulk non-CLI
  imports, launch commands that could not be verified.

No depth test required beyond doc/checklist assertions from P10 and the
focused final acceptance matrix.

## Error-Footer Canonical Pairs

| Error | `try:` |
|---|---|
| CLI binary missing | `try: <descriptor install_hint.try_lines[0]>` |
| Descriptor has `[ingest]` but unknown schema | `try: update provider descriptor schema or disable ingest for <id>` |
| Ingest root missing | No hard failure; show no activity and let `detect` report filesystem artifacts |
| Generic CLI descriptor missing `exec_template.args_template` | `try: add [exec_template] args_template = ["...", "{prompt}"]` |
| Launch command unverified | `try: keep provider detect/ingest-only until the CLI invocation is documented` |

Parameterized tests should exercise the unknown-schema and missing-template
cases.

## Out Of Scope

- Provider-aware undo/truncation of native transcripts.
- OpenCode SQLite unless dependency/build impact is explicitly accepted during
  P9.
- Importing agentsview as a crate/module.
- Full `ParsedSession` analytics, fork detection, or replay UI.
- Launch descriptors for CLIs whose noninteractive command shape is not
  verified.
- Any network-based probe in default `detect`; version probes stay local unless
  the user opts into pings.

## Dependencies

- **Tier 1 expected:** none beyond current workspace crates.
- **Tier 2 possible:** SQLite support for OpenCode, only if P9 explicitly
  accepts it and logs the dependency in `DEPENDENCIES.md`.
- **Tier 3 blocked:** embedding agentsview or shelling out to it.

## Engineering Invariants

- Depth tests before implementation in every phase.
- Keep existing Codex/Claude behavior compatible unless a test is explicitly
  renamed and the CHANGELOG calls out the changed TUI wording.
- No wildcard arms in schema/cwd-match tests where exhaustiveness matters.
- No private transcript fixtures. Use minimal synthetic JSON/JSONL.
- Focused verification green at every commit. Full `make verify`, release
  builds, smoke, and stress are not default gates for this rider.
