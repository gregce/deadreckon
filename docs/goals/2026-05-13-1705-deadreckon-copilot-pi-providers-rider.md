# deadreckon - Copilot and Pi Providers Rider

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-13-1705-deadreckon-copilot-pi-providers-goal.md`.
It supersedes nothing in prior riders, especially
`2026-05-13-1345-deadreckon-provider-cli-ingest-rider.md`; those invariants
still apply. This rider adds two concrete descriptor-backed CLI providers:
GitHub Copilot CLI and Pi.

**All paths absolute.** Source `/Users/gdc/deadreckon/`; research
references `/Users/gdc/gnhf/` and `/Users/gdc/agentsview/`; runtime
`~/.deadreckon/`, `~/.copilot/`, and `~/.pi/agent/`.

## Posture (decided - do not redesign)

- **Maturity stays `alpha`.**
- **No `PipelineState` schema changes.** Provider descriptors, parser dispatch,
  and focused tests may change.
- **Provider-owned transcripts are read-only.** Do not truncate, rewrite,
  migrate, or delete files under `~/.copilot` or `~/.pi`.
- **No runtime dependency on `/Users/gdc/gnhf` or `/Users/gdc/agentsview`.**
  Use them as research/spec only.
- **Prefer descriptors and generic CLI launch.** Add a concrete provider only if
  Pi cannot be launched reliably with prompt-as-argument and a small generic
  transport extension would be riskier.
- **No live model calls in automated tests.** Use fake binaries and fixtures.
- **No `git push`.** Phased local commits only.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Verification Ladder (focused, not full-suite)

Do not run these by default during this goal:

- `make verify`
- `cargo build --release`
- `cargo test --workspace`
- `make smoke`
- `make stress`

Per phase, run the named depth tests plus the smallest relevant checks:

```zsh
cargo test -p deadreckon-providers --test registry <filter>
cargo test -p deadreckon-providers --test cli_providers <filter>
cargo test -p deadreckon --test providers_list <filter>
cargo test -p deadreckon --test detect <filter>
cargo test -p deadreckon provider_jsonl
cargo fmt --check
cargo clippy -p deadreckon-providers --tests -- -D warnings
cargo clippy -p deadreckon --tests -- -D warnings
```

Final acceptance matrix:

```zsh
cargo test -p deadreckon-providers --test registry
cargo test -p deadreckon-providers --test cli_providers
cargo test -p deadreckon --test providers_list
cargo test -p deadreckon --test detect
cargo test -p deadreckon provider_jsonl
cargo fmt --check
cargo clippy -p deadreckon-providers --tests -- -D warnings
cargo clippy -p deadreckon --tests -- -D warnings
```

Only run the long suite if the user explicitly requests it. If skipped, say so
in the handoff and list the focused commands that ran.

## Research Facts To Preserve

### GitHub Copilot CLI

Local binary: `/Users/gdc/.npm-global/bin/copilot`.

Local `copilot --help` confirmed:

- `-p, --prompt <text>` for non-interactive prompt.
- `--output-format json` emits JSONL, one JSON object per line.
- `--stream on|off`, `--no-color`, `--allow-all`, `--model`, `--log-dir`.
- Default operational logs: `~/.copilot/logs/`.

GNHF at `/Users/gdc/gnhf/src/core/agents/copilot.ts` launches:

```text
copilot -p <augmented prompt> --output-format json --stream off --no-color --allow-all
```

and parses JSONL:

- `assistant.message.data.content`
- `assistant.message.data.outputTokens`
- top-level `usage` with camelCase or snake_case token fields.

Agentsview at `/Users/gdc/agentsview/internal/parser/` uses the session logs,
not the operational debug logs:

- root env/config: `COPILOT_DIR`, default `.copilot`.
- session files:
  - `~/.copilot/session-state/<uuid>.jsonl`
  - `~/.copilot/session-state/<uuid>/events.jsonl`
- event types: `session.start`, `user.message`, `assistant.message`,
  `assistant.reasoning`, `tool.execution_complete`, `session.model_change`.
- cwd: `session.start.data.context.cwd`.

### Pi

Local binary: `/Users/gdc/.npm-global/bin/pi`.

Local `pi --help` confirmed:

- `--mode text|json|rpc`; JSON mode is in scope.
- `--print, -p` for non-interactive prompt.
- `--session-dir <dir>`, `--no-session`, `--model`, `--provider`.
- env roots: `PI_CODING_AGENT_DIR`, `PI_CODING_AGENT_SESSION_DIR`.
- built-in tools: `read`, `bash`, `edit`, `write`, `grep`, `find`, `ls`.

GNHF at `/Users/gdc/gnhf/src/core/agents/pi.ts` launches:

```text
pi --mode json --no-session
```

then writes the augmented prompt to stdin and parses JSONL streaming events:

- `message_update`
- `message_end`
- `turn_end`
- `agent_end`
- `message.usage.input`, `message.usage.output`, `cacheRead`, `cacheWrite`.

Agentsview at `/Users/gdc/agentsview/internal/parser/pi.go` parses saved Pi
session JSONL:

- first nonblank line must be `{"type":"session", ...}`.
- header fields include `id`, `cwd`, `timestamp`, optional `branchedFrom`.
- message rows use `type = "message"` and `message.role` of `user`,
  `assistant`, or `toolResult`.
- `model_change.modelId` sets current model.
- assistant content blocks include `text`, `thinking`, and `toolCall`.
- token usage lives under `message.usage`.

Local Pi sessions have been observed under:

```text
/Users/gdc/.pi/agent/sessions/<encoded-cwd>/*.jsonl
```

## Descriptor Targets

### `cli:copilot`

Create `/Users/gdc/deadreckon/crates/deadreckon-providers/descriptors/cli-copilot.toml`.

```toml
id = "cli:copilot"
display_name = "GitHub Copilot CLI"
kind = "cli"
default_binary = "copilot"
allow_network_default = true
default_model = "provider default"
sandbox_writes = ["~/.copilot"]
sandbox_reads = ["~/.copilot"]
fs_detection_paths = ["~/.copilot"]
docs_url = "https://docs.github.com/copilot"
subscription = true

[auth]
kind = "subscription"

[version_probe]
args = ["--help"]
expect_substring = "GitHub Copilot CLI"

[exec_template]
args_template = ["-p", "{prompt}", "--output-format", "json", "--stream", "off", "--no-color", "--allow-all"]
model_arg = "--model"
timeout_seconds = 1800

[install_hint]
url = "https://docs.github.com/copilot"
try_lines = ["npm install -g @github/copilot"]

[ingest]
id_prefix = "copilot:"
env_var = "COPILOT_DIR"
default_dirs = ["~/.copilot"]
watch_subdirs = ["session-state"]
schema = "copilot-cli"
cwd_match = "json-pointer"
cwd_match_path = "data.context.cwd"
file_glob = "*.jsonl"
freshness_minutes = 2
storage = "jsonl"
```

Notes:

- `~/.copilot/logs` is diagnostic only. TUI transcript activity should use
  `session-state`.
- Recursive file discovery already sees both bare `*.jsonl` and nested
  `events.jsonl`; add a specific test so this does not regress.
- If `COPILOT_DIR` points at the app root, the current recursive collector is
  enough. Do not reinterpret it as a session-state directory unless a test
  covers both forms.

### `cli:pi`

Create `/Users/gdc/deadreckon/crates/deadreckon-providers/descriptors/cli-pi.toml`.

Default should preserve session logs so the attach TUI can find them. Start
with prompt-as-argument launch because local help documents it:

```toml
id = "cli:pi"
display_name = "Pi CLI"
kind = "cli"
default_binary = "pi"
allow_network_default = true
default_model = "provider default"
sandbox_writes = ["~/.pi/agent"]
sandbox_reads = ["~/.pi/agent"]
fs_detection_paths = ["~/.pi/agent/sessions"]
docs_url = "https://pi.dev/"
subscription = true

[auth]
kind = "subscription"

[version_probe]
args = ["--help"]
expect_substring = "AI coding assistant"

[exec_template]
args_template = ["--mode", "json", "--print", "{prompt}"]
model_arg = "--model"
timeout_seconds = 1800

[install_hint]
url = "https://pi.dev/"
try_lines = ["npm install -g @earendil-works/pi-coding-agent"]

[ingest]
id_prefix = "pi:"
env_var = "PI_CODING_AGENT_SESSION_DIR"
default_dirs = ["~/.pi/agent/sessions"]
schema = "pi"
cwd_match = "top-level"
file_glob = "*.jsonl"
freshness_minutes = 2
storage = "jsonl"
```

Notes:

- Do not include `--no-session` in the default descriptor if TUI session ingest
  depends on saved Pi files.
- If `pi --mode json --print {prompt}` is not reliable with a fake or local
  dry-run binary, add a minimal generic CLI prompt transport:
  `prompt_transport = "arg" | "stdin"` defaulting to `arg`, then make Pi use
  stdin. This is a descriptor schema change and needs round-trip tests.
- If the final implementation uses `--session-dir`, prefer a stable provider
  log directory that the descriptor can express. If it requires `{run_root}`,
  log that as a V1 candidate unless the change is tiny and tested.

## Parser Requirements

### Common output rows

Both schemas must feed the existing `ProviderActivity` surface:

- user/assistant text: `agent <summary>`
- thinking: `thinking <summary>` or the existing local convention.
- tool requests: `tool <normalized category/name>`
- tool results: `result <summary or byte count>`
- token/context: update `context_tokens` and `context_window` when known.

Do not invent a new TUI panel. The existing provider activity panel should work.

### `copilot-cli` parser

Line-based parser in `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs`.

Handle:

- `session.start`: record cwd match only; no row required.
- `session.model_change`: update model context if the existing activity surface
  has a place to show it; otherwise ignore.
- `user.message`: optional row only if existing providers show user text.
- `assistant.message`:
  - `data.content` -> agent row.
  - `data.reasoningText` -> thinking row when present.
  - `data.toolRequests[]` -> tool rows with taxonomy normalization.
  - `data.outputTokens` -> token telemetry.
- `assistant.reasoning`: mark thinking if no text is present.
- `tool.execution_complete`: result row using `data.result` length/summary.
- top-level `usage`: accept both camelCase and snake_case fields.

Depth fixtures must include one bare session file and one nested
`events.jsonl` file.

### `pi` parser

Line-based parser in `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs`.

Handle:

- first `type = "session"` line with top-level `cwd` for matching.
- `type = "model_change"` with `modelId`.
- `type = "message"`:
  - `message.role = "user"` -> optional user row.
  - `message.role = "assistant"` -> parse content string or content blocks.
  - `message.role = "toolResult"` -> result row.
- assistant content blocks:
  - `text.text` -> agent row.
  - `thinking.thinking` -> thinking row; empty thinking still counts as a
    thinking marker if the TUI has that concept.
  - `toolCall.name` and `toolCall.arguments` -> tool row.
- token usage:
  - `message.usage.input`
  - `message.usage.output`
  - `message.usage.cache.read`
  - `message.usage.cache.write`
  - `message.usage.cacheRead`
  - `message.usage.cacheCreation`

Normalize Pi's `agent__intent` and `_i` argument fields to `description`
before rendering tool details, matching agentsview's parser behavior.

## Phases

Each phase writes the named depth test first, watches it fail, implements, then
runs the phase's focused commands. Commit locally after each green phase.

### P1 - Registry built-ins

- Add Copilot and Pi descriptors to the built-in descriptor source list.
- Keep provider IDs exactly `cli:copilot` and `cli:pi`.
- No routing changes yet beyond descriptor loading.

Depth tests in `/Users/gdc/deadreckon/crates/deadreckon-providers/tests/registry.rs`:

- `registry_builtin_lists_copilot_and_pi`
- `descriptor_ingest_round_trips_for_copilot_and_pi`
- `descriptor_models_and_install_hints_cover_copilot_and_pi`

Focused verification:

```zsh
cargo test -p deadreckon-providers --test registry copilot
cargo test -p deadreckon-providers --test registry pi
```

### P2 - Detect and provider listing UX

- Ensure `deadreckon detect` includes both providers with useful failed/probed
  status when binaries are absent.
- Ensure `deadreckon providers list --all` includes both.
- Keep output formatting compatible with existing tests.

Depth tests:

- `/Users/gdc/deadreckon/crates/deadreckon/tests/detect.rs`
  - `detect_lists_copilot_and_pi_descriptors_with_install_hints`
- `/Users/gdc/deadreckon/crates/deadreckon/tests/providers_list.rs`
  - `providers_list_all_includes_copilot_and_pi`

Focused verification:

```zsh
cargo test -p deadreckon --test detect copilot
cargo test -p deadreckon --test providers_list copilot
```

### P3 - Generic CLI routing for Copilot

- Fake binary proves rendered args:
  `-p <prompt> --output-format json --stream off --no-color --allow-all`.
- Model override inserts `--model <model>` before the prompt per current generic
  behavior, unless local tests prove Copilot requires a different position.
- Response content remains stdout and trace marks `kind = "cli_subagent"`.

Depth tests in `/Users/gdc/deadreckon/crates/deadreckon-providers/tests/cli_providers.rs`:

- `generic_cli_provider_runs_builtin_copilot_descriptor`
- `generic_cli_provider_passes_copilot_model_arg`

Focused verification:

```zsh
cargo test -p deadreckon-providers --test cli_providers copilot
```

### P4 - Generic CLI routing for Pi

- Fake binary proves rendered args for the chosen descriptor launch.
- If prompt-as-argument fails by design, add descriptor `prompt_transport` with
  `arg` default and `stdin` support. Update fake binary tests for both modes.
- Do not call a live model.

Depth tests:

- `generic_cli_provider_runs_builtin_pi_descriptor`
- `generic_cli_provider_passes_pi_model_arg`
- If needed: `generic_cli_provider_can_write_prompt_to_stdin`

Focused verification:

```zsh
cargo test -p deadreckon-providers --test cli_providers pi
```

### P5 - Copilot discovery and cwd matching

- Use existing recursive candidate discovery, but depth-test both known Copilot
  layouts.
- Confirm `cwd_match = "json-pointer"` with `data.context.cwd` matches
  `session.start` rows.
- Ensure stale files are filtered by freshness.

Depth tests in `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs` test module:

- `copilot_ingest_discovers_bare_session_state_jsonl`
- `copilot_ingest_discovers_nested_events_jsonl`
- `copilot_ingest_json_pointer_cwd_matches_session_start`

Focused verification:

```zsh
cargo test -p deadreckon provider_jsonl_copilot
```

### P6 - Copilot TUI activity parser

- Add schema dispatch for `copilot-cli`.
- Parse assistant text, reasoning, tool requests, tool completions, and usage.
- Keep activity cap and provider log footer behavior unchanged.

Depth tests:

- `copilot_activity_parses_assistant_message_and_usage`
- `copilot_activity_parses_tool_request_and_result`
- `copilot_activity_ignores_unrelated_event_rows`

Focused verification:

```zsh
cargo test -p deadreckon provider_jsonl_copilot
```

### P7 - Pi discovery and cwd matching

- Use Pi's session root, not the app root, as default ingest root.
- Validate first nonblank JSONL row has `type = "session"` before accepting a
  file, or make the parser silently ignore non-Pi JSONL files.
- Match top-level `cwd` from the session header.

Depth tests:

- `pi_ingest_discovers_session_jsonl_under_encoded_cwd_dir`
- `pi_ingest_rejects_jsonl_without_session_header`
- `pi_ingest_top_level_cwd_matches_session_header`

Focused verification:

```zsh
cargo test -p deadreckon provider_jsonl_pi
```

### P8 - Pi TUI activity parser

- Add schema dispatch for `pi`.
- Parse string content and content blocks.
- Parse tool calls/results and normalize `agent__intent` / `_i` to
  `description`.
- Extract context/output/cache token telemetry.

Depth tests:

- `pi_activity_parses_text_thinking_tool_and_result_blocks`
- `pi_activity_normalizes_intent_argument_description`
- `pi_activity_extracts_usage_context_tokens`

Focused verification:

```zsh
cargo test -p deadreckon provider_jsonl_pi
```

### P9 - Runtime output names and attach smoke

- Ensure `provider_output_name("cli:copilot") == "copilot.out"` and
  `provider_output_name("cli:pi") == "pi.out"`.
- If Pi defaults to `--no-session` for any reason, add run-output ingest or log
  the gap in `docs/V1-CANDIDATES.md`; do not pretend TUI logs work without a
  fixture.

Depth tests:

- `provider_output_name_slugifies_new_cli_descriptor_ids`
- `attach_activity_uses_copilot_fixture_for_active_provider`
- `attach_activity_uses_pi_fixture_for_active_provider`

Focused verification:

```zsh
cargo test -p deadreckon provider_output_name
cargo test -p deadreckon provider_jsonl_copilot
cargo test -p deadreckon provider_jsonl_pi
```

### P10 - Error footers and local probe polish

- Error messages for missing binaries must include install `try:` lines.
- If `pi --help` attempts state writes in normal sandboxed tests, avoid real Pi
  in tests and document that local probes may need home write access.
- Confirm configured provider override can replace binary paths for both.

Depth tests:

- `missing_copilot_binary_reports_install_hint`
- `missing_pi_binary_reports_install_hint`
- `provider_override_can_replace_copilot_and_pi_binary_paths`

Focused verification:

```zsh
cargo test -p deadreckon-providers --test registry override
cargo test -p deadreckon --test detect copilot
cargo test -p deadreckon --test detect pi
```

### P11 - Architecture docs and changelog

- Update `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:
  - §13 provider descriptors / routing: add Copilot and Pi.
  - §18 TUI attach: add Copilot/Pi ingest source and schema notes.
  - §22 built-vs-thin: mark provider breadth expanded; note any deferred
    stdin/run-output/session-dir issue honestly.
- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:

```markdown
## Copilot and Pi providers (alpha) - 2026-05-13

- Added descriptor-backed GitHub Copilot CLI and Pi providers with focused
  registry, routing, detection, and TUI ingest coverage.
```

- Add any deferred items to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

No depth test unless docs assertion tests already cover this section.

Focused verification:

```zsh
cargo fmt --check
```

## Integration Matrix

| Surface | Copilot | Pi |
|---|---|---|
| Built-in descriptor | `cli:copilot` | `cli:pi` |
| Binary | `copilot` | `pi` |
| Launch | `-p {prompt} --output-format json --stream off --no-color --allow-all` | `--mode json --print {prompt}` or tested stdin transport |
| Model flag | `--model` | `--model` |
| State root | `~/.copilot` | `~/.pi/agent` |
| Session logs | `session-state/*.jsonl`, `session-state/*/events.jsonl` | `sessions/<encoded-cwd>/*.jsonl` |
| Cwd match | `data.context.cwd` | top-level `cwd` in session header |
| Parser schema | `copilot-cli` | `pi` |
| Test source | fixture JSONL + fake binary | fixture JSONL + fake binary |

## Error-footer Canonical Pairs

| Error | `try:` |
|---|---|
| Copilot binary missing | `npm install -g @github/copilot` |
| Pi binary missing | `npm install -g @earendil-works/pi-coding-agent` |
| Copilot descriptor has unknown ingest schema | `try: update provider descriptor schema or disable ingest for cli:copilot` |
| Pi descriptor has unknown ingest schema | `try: update provider descriptor schema or disable ingest for cli:pi` |
| Pi prompt transport not supported | `try: use a provider override with prompt-as-arg or implement descriptor stdin transport` |

## Dependencies

Tier 1 (utility, free): none expected.

Tier 2 (architectural): none expected. If stdin prompt transport needs a crate,
stop and justify it in `docs/V1-CANDIDATES.md`; prefer standard library/Tokio.

Tier 3 (blocked): any dependency that adds native build, network-only tests, or
provider SDK coupling.

## Out of scope

- Live Copilot or Pi model calls in CI or automated tests.
- Rewriting, compacting, or deleting Copilot/Pi native session files.
- Copilot operational log parsing under `~/.copilot/logs` beyond noting the
  path in docs.
- Pi extension/package management.
- VS Code Copilot parser support; this rider is GitHub Copilot CLI only.
- A generalized multi-provider transcript database.
- Full workspace verification unless the user explicitly asks.

## Engineering invariants

- Descriptor-backed providers remain preferred; no new `ProviderKind` variants
  unless a fake-binary test proves generic launch cannot support the provider.
- Existing Codex, Claude, Gemini, and OpenCode descriptor behavior must not
  regress.
- Parser fixtures must be small, local, and free of real private prompt text.
- Provider-owned logs are read-only.
- TUI activity row vocabulary remains shared across providers.
- No silent scope expansion; V1 questions go to `docs/V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push`.
- One depth test before each phase implementation.
- Each phase runs only the focused verification relevant to touched files.
- Each commit mentions the focused commands that ran.
- Final handoff states explicitly that the long suite was skipped unless the
  user asked for it.
