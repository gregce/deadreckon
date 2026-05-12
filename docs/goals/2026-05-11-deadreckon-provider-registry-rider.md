# deadreckon — Provider Registry Rider (mine specstory-cli's SPI shape)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-11-deadreckon-provider-registry-goal.md`.
It supersedes nothing in prior riders
(`2026-05-10-deadreckon-build-rider.md`,
`2026-05-11-deadreckon-primary-flow-rider.md`,
`2026-05-11-deadreckon-robust-rider.md`,
`2026-05-11-deadreckon-usability-rider.md`,
`2026-05-11-deadreckon-orchestrate-rider.md`,
`2026-05-11-deadreckon-codebase-rider.md`,
`2026-05-11-deadreckon-self-documenting-rider.md`,
`2026-05-11-deadreckon-audit-harden-rider.md`,
`2026-05-11-deadreckon-doc-depth-rider.md`) — their invariants,
sandbox defaults, files-not-fields posture, error-footer convention,
and existing verbs still apply. This rider adds: a registry-driven
provider layer (`ProviderDescriptor` + `ProviderRegistry`), a
`detect` verb, three new CLI providers (`cursor-agent`, `gemini`,
`aider`), two new HTTP providers (Gemini API, Ollama), and a model
catalog with `--model` resolution.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided — do not redesign)

- **Maturity stays `alpha`.** Workspace stays `version = "0.1.0"`.
- **No `PipelineState` schema changes.** Descriptors live in files —
  built-in defaults compiled into the binary, user overrides at
  `~/.deadreckon/providers.d/<id>.toml`. No new state struct fields.
- **Backwards-compatible.** Existing `config.toml` keys
  (`default_provider`, `fallback`, `[providers.<id>]`) work
  unchanged. Existing tests pass without modification. The legacy
  `ProviderKind` enum variants stay; a new `Generic` variant covers
  descriptor-driven dispatch.
- **specstory-cli is the inspiration, not the import.** We mine the
  shape (`Check(customCommand)`, registry, per-provider actionable
  errors); the implementation is Rust-idiomatic and integrates with
  deadreckon's existing `Provider` trait + `ProviderRouter`.
- **No new top-level workspace crate.** The registry lives inside
  `crates/deadreckon-providers/src/registry/`.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Bedrock (signed AWS auth), Vertex (Google
  IAM), MCP-server providers, and provider-side hooks/middleware all
  go in `docs/V1-CANDIDATES.md`. If a phase reveals a major
  architectural decision, log it and continue.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## What we mined from specstory-cli (the deltas worth porting)

Concrete patterns observed in `/Users/gdc/specstory-cli/pkg/`:

| Pattern | Source | What we adopt |
|---|---|---|
| **`Provider` interface with `Check(customCommand) → CheckResult{Success,Version,Location,ErrorMessage}`** | `pkg/spi/provider.go:27-77` | A new `ProviderProbe` trait alongside our existing `Provider` trait; descriptor-driven default impl. |
| **Single-file registry; one-line per addition** | `pkg/spi/factory/registry.go:46-77` | `register!()` macro / Rust array of built-in descriptors in `registry/builtin.rs`. |
| **`SplitCommandLine` for user-supplied custom commands** | `pkg/spi/cmdline.go:23-80` | A `parse_custom_command(s) → (binary, args)` helper in the registry crate. |
| **Per-provider actionable error messages with install/troubleshoot hints** | `pkg/providers/claudecode/provider.go:111-158` | Descriptor carries `install_hint{url, try_lines}`; the registry formats the canonical `try:` error footer. |
| **Filesystem detection beyond PATH** (e.g., `~/.claude/projects/<dir-hash>/`) | `pkg/providers/claudecode/provider.go:264-295` | Descriptor carries `fs_detection_paths: Vec<PathBuf>`; `detect` walks them. |
| **Custom-command override per invocation** | `pkg/providers/codexcli/provider.go:111-184` | `--provider-binary <id>=<path>` flag and per-provider `binary` config (already partially present). |

We do **not** port: specstory-cli's `WatchAgent` / `GetAgentChatSession*`
(those belong to deadreckon's `import` verb, which already covers
claude/codex/cursor histories — out of scope here).

## Data model (files-not-fields)

### `ProviderDescriptor` (in-memory; serialized to TOML for overrides)

```rust
pub struct ProviderDescriptor {
    pub id: String,                              // "anthropic" | "cli:codex" | "gemini" | ...
    pub display_name: String,                    // "Anthropic API" | "Codex CLI" | ...
    pub kind: DescriptorKind,                    // Http | Cli | LocalHttp | Scripted
    pub default_binary: Option<String>,          // CLI: "codex"; HTTP: None
    pub default_endpoint: Option<String>,        // HTTP: "https://api.anthropic.com"; CLI: None
    pub auth: AuthShape,                         // ApiKey{env_var} | None | Subscription
    pub version_probe: Option<VersionProbe>,     // CLI: ["-v" | "--version"]; HTTP: ping endpoint
    pub exec_template: ExecTemplate,             // request shape (see below)
    pub sandbox_writes: Vec<PathBuf>,            // CLI sandbox write allowlist (~/.codex, etc.)
    pub sandbox_reads: Vec<PathBuf>,             // CLI sandbox read allowlist
    pub allow_network_default: bool,             // honors audit-harden's tool-policy default ON
    pub model_catalog: Vec<ModelEntry>,          // [{id, ctx_window, input_per_million, output_per_million}]
    pub default_model: Option<String>,           // model id from catalog
    pub fs_detection_paths: Vec<PathBuf>,        // ~/.claude/projects/, ~/.codex/sessions/, etc.
    pub install_hint: InstallHint,               // {url, try_lines: Vec<String>}
    pub docs_url: Option<String>,
    pub subscription: bool,                      // true: wall-clock spend; false: token spend
}

pub enum DescriptorKind { Http, Cli, LocalHttp, Scripted }

pub enum AuthShape {
    ApiKey { env_var: String, header: String, scheme: AuthScheme },
    None,                                        // local Ollama, smoke
    Subscription,                                // CLI providers
}

pub enum AuthScheme { Bearer, XApiKey, Basic, Custom(String) }

pub struct VersionProbe {
    pub args: Vec<String>,                       // ["-v"], ["--version"]
    pub expect_substring: Option<String>,        // "(Claude Code)" must appear in output
    pub min_known_good: Option<String>,          // "0.5.0"; warn below
}

pub struct ExecTemplate {
    pub args_template: Vec<String>,              // tokens with {prompt} {model} {sandbox} placeholders
    pub model_arg: Option<String>,               // "--model"; None → no per-call model override
    pub timeout_seconds: Option<u64>,            // CLI: per-turn cap; HTTP: request cap
    pub request_shape: Option<RequestShape>,     // HTTP only
}

pub enum RequestShape { OpenAiChat, AnthropicMessages, GeminiGenerateContent, OllamaChat }

pub struct ModelEntry {
    pub id: String,                              // "gpt-4o-mini" | "claude-sonnet-4-5"
    pub context_window: Option<u32>,
    pub input_per_million: Option<f64>,
    pub output_per_million: Option<f64>,
    pub aliases: Vec<String>,                    // user-typeable shortnames
}

pub struct InstallHint {
    pub url: String,                             // canonical install doc
    pub try_lines: Vec<String>,                  // formatted as `try: <line>`
}
```

### Override file shape (`~/.deadreckon/providers.d/<id>.toml`)

```toml
# Override or extend a built-in descriptor. Absent fields fall through
# to the built-in. New IDs (not built-in) register a new provider.
id = "cli:claude-code"
display_name = "Claude Code (custom path)"
default_binary = "/opt/homebrew/bin/claude"
default_model = "claude-opus-4-1"

[[model_catalog]]
id = "claude-opus-4-1"
context_window = 200000
input_per_million = 15.0
output_per_million = 75.0
aliases = ["opus", "opus-4"]

[install_hint]
url = "https://docs.claude.com/en/docs/claude-code/quickstart"
try_lines = ["brew install claude", "npm i -g @anthropic-ai/claude-code"]
```

`<id>.toml` files are loaded in lexical order; later files override
earlier. Built-ins compile in via `include_str!` of TOML files at
`crates/deadreckon-providers/descriptors/<id>.toml` so the source of
truth is text, not Rust literals.

### `detect` output (per-provider)

```
cli:codex     ✓  /opt/homebrew/bin/codex          v0.5.4   (subscription)
cli:claude-code ✗ not on PATH                              (try: brew install claude)
anthropic     ✓  https://api.anthropic.com         ANTHROPIC_API_KEY set  (metered)
openai        ✗ ANTHROPIC_API_KEY missing                  (try: export OPENAI_API_KEY=sk-...)
gemini        ✗ GEMINI_API_KEY missing                     (try: export GEMINI_API_KEY=...)
ollama        ✓  http://localhost:11434           reachable             (local, no key)
```

Machine-readable form (`--json`):

```json
{
  "providers": [
    {
      "id": "cli:codex",
      "kind": "cli",
      "status": "ok",
      "location": "/opt/homebrew/bin/codex",
      "version": "0.5.4",
      "subscription": true,
      "fs_artifacts": ["~/.codex/sessions/"]
    },
    ...
  ]
}
```

## Built-in descriptor inventory (this rider's deliverable)

| id | kind | source descriptor file | Notes |
|---|---|---|---|
| `anthropic` | Http | `descriptors/anthropic.toml` | migrated; `request_shape = AnthropicMessages` |
| `openai` | Http | `descriptors/openai.toml` | migrated; `request_shape = OpenAiChat` |
| `openai-compatible` | Http | `descriptors/openai-compatible.toml` | migrated; ID-aliased per user route |
| `smoke` | Scripted | `descriptors/smoke.toml` | migrated; `auth = None` |
| `cli:claude-code` | Cli | `descriptors/cli-claude-code.toml` | migrated |
| `cli:codex` | Cli | `descriptors/cli-codex.toml` | migrated |
| `cli:cursor-agent` | Cli | `descriptors/cli-cursor-agent.toml` | NEW (P5) |
| `cli:gemini` | Cli | `descriptors/cli-gemini.toml` | NEW (P6) |
| `cli:aider` | Cli | `descriptors/cli-aider.toml` | NEW (P7) |
| `gemini` | Http | `descriptors/gemini.toml` | NEW (P8); `request_shape = GeminiGenerateContent` |
| `ollama` | LocalHttp | `descriptors/ollama.toml` | NEW (P9); `auth = None`, default `http://localhost:11434` |

V1 (out of scope — `docs/V1-CANDIDATES.md` entries):
`cli:opencode`, `cli:goose`, `cli:amp`, `cli:q` (Amazon Q),
`cli:crush`, `bedrock` (signed AWS), `vertex` (Google IAM), `mistral`,
`grok` (xAI), `together`, `groq`, `openrouter` (latter three already
work via `openai-compatible` — V1 is shipping curated descriptors with
model catalogs).

## CLI provider exec templates (the spec)

Each CLI descriptor's `exec_template.args_template` is the source of
truth for invocation. Templates use `{prompt}`, `{model}`, `{sandbox}`,
`{cwd}` placeholders; the registry substitutes at exec time.

### `cli:claude-code` (migrated)

```toml
[exec_template]
args_template = ["--dangerously-skip-permissions", "-p", "{prompt}"]
model_arg = "--model"  # optional; not currently exercised
```

### `cli:codex` (migrated)

```toml
[exec_template]
args_template = [
  "--ask-for-approval", "never",
  "exec",
  "--skip-git-repo-check",
  "--sandbox", "{sandbox}",
  "{prompt}",
]
model_arg = "--model"
```

`{sandbox}` is `workspace-write` when no outer sandbox is active,
`danger-full-access` otherwise (matches current
`codex_sandbox_mode()`).

### `cli:cursor-agent` (NEW, P5)

```toml
[exec_template]
args_template = ["--print", "{prompt}"]   # cursor-agent --print is the non-interactive form
model_arg = "--model"
timeout_seconds = 1800

sandbox_writes = ["~/.cursor"]
sandbox_reads = ["~/.cursor"]
fs_detection_paths = ["~/.cursor/chats", "~/Library/Application Support/Cursor"]

[install_hint]
url = "https://cursor.com/cli"
try_lines = ["curl https://cursor.com/install | bash"]
```

### `cli:gemini` (NEW, P6)

```toml
[exec_template]
args_template = ["--prompt", "{prompt}"]
model_arg = "--model"

sandbox_writes = ["~/.gemini", "~/.config/gemini"]
fs_detection_paths = ["~/.gemini/chats"]

[install_hint]
url = "https://ai.google.dev/gemini-cli/get-started"
try_lines = ["npm i -g @google/gemini-cli"]
```

### `cli:aider` (NEW, P7)

```toml
[exec_template]
args_template = ["--no-auto-commits", "--yes", "--message", "{prompt}"]
model_arg = "--model"

sandbox_writes = ["~/.aider"]
fs_detection_paths = ["~/.aider/chats", ".aider.chat.history.md"]

[install_hint]
url = "https://aider.chat/docs/install.html"
try_lines = ["pipx install aider-chat", "pip install -U aider-chat"]
```

CLI-provider depth tests use **fake binaries** in `tests/bin/`
that print known stdout (mirroring the existing
`crates/deadreckon-providers/tests/cli_providers.rs` pattern); we do
not require the real binaries to be installed for `cargo test`.

## HTTP descriptors

### `gemini` (Google AI Studio API, NEW, P8)

```toml
default_endpoint = "https://generativelanguage.googleapis.com"

[auth]
kind = "ApiKey"
env_var = "GEMINI_API_KEY"
header = "x-goog-api-key"
scheme = "Custom"

[exec_template]
request_shape = "GeminiGenerateContent"
# Path template: /v1beta/models/{model}:generateContent
```

Adapter parses `{candidates: [{content: {parts: [{text}]}}], usageMetadata: {promptTokenCount, candidatesTokenCount}}`.

### `ollama` (local, NEW, P9)

```toml
default_endpoint = "http://localhost:11434"

[auth]
kind = "None"

[exec_template]
request_shape = "OllamaChat"
# Path: /api/chat
allow_network_default = true   # local; no internet
```

Has `has_credential()` succeed when the endpoint responds to a
GET `/api/tags` within 2 s; no API key needed.

## `--model` resolution algorithm

```
fn resolve_model_to_provider(model: &str, registry: &Registry, config: &Config) -> Result<&Descriptor> {
    let candidates: Vec<&Descriptor> = registry.iter()
        .filter(|d| d.has_credential() || d.kind == DescriptorKind::Scripted)
        .filter(|d| d.model_catalog.iter().any(|m|
            m.id == model || m.aliases.iter().any(|a| a == model)
        ))
        .collect();
    match candidates.len() {
        0 => Err(NoCandidate { model, suggested: nearest_models(model, registry) }),
        1 => Ok(candidates[0]),
        _ => {
            // tiebreak: explicit `default_provider` first, then config fallback order, then descriptor id alphabetical.
            Ok(pick_per_tiebreak(candidates, config))
        }
    }
}
```

Refusal cases:

| Condition | Error | `try:` |
|---|---|---|
| Model unknown to registry | `unknown model '<m>'; nearest: <a>, <b>` | `deadreckon providers list --models` |
| Multiple candidates | `model '<m>' available via [<a>, <b>]; ambiguous` | `--provider <a>` or set `default_provider` |
| No credentialed candidate | `model '<m>' known but no credentialed provider` | `<install_hint>` from the matching descriptors |

## Verb signatures

```
deadreckon detect [<id>]
    [--json]                        # machine-readable
    [--ping]                        # also probe HTTP endpoints (cost: ~1 token per provider)
deadreckon providers list
    [--models]                      # also list each provider's model catalog
    [--all]                         # include built-ins not currently configured
    [--full]                        # full IDs/paths for scripts
deadreckon run "..." [--model <id>] # P10: --model flag resolves through the registry
deadreckon config restore           # P11: revert to last-known-good config (already in scope from prior riders if not, this is the addition here)
```

`detect`'s default form is per-provider one-line; `--json` emits the
machine schema above. `--ping` is gated to avoid spending HTTP credits
during routine `doctor` runs.

Refusal cases for `detect`:

| Condition | Error | `try:` |
|---|---|---|
| Provider id unknown | `no provider '<id>' in registry` | `deadreckon providers list` |
| Probe failed (binary not on PATH) | `<id>: not on PATH` | `<install_hint>` |
| Probe failed (binary present, version mismatch) | `<id>: version <X> below known-good <Y>` | `<install_hint>` (upgrade line) |
| Probe failed (HTTP 401/403) | `<id>: auth failed (<env_var> set?)` | `export <env_var>=...` |
| Probe failed (HTTP timeout) | `<id>: endpoint <url> timed out` | `--ping --timeout <secs>` |

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them
fail; implement; green on
`cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`;
conventional-commit local commit; one-line CHANGELOG entry.

### P1 — Descriptor type + registry plumbing

- New module `crates/deadreckon-providers/src/registry/mod.rs` with
  the types in §"Data model" above.
- `descriptors/<id>.toml` files for the six existing providers (one
  per current adapter) compiled in via `include_str!`.
- `ProviderRegistry::builtin()` returns a `Registry` populated from
  the compiled-in descriptors.
- `ProviderRegistry::with_overrides(home)` walks
  `<home>/providers.d/*.toml` in lexical order, merging.
- `parse_custom_command(s) -> (binary, args)` helper.

Depth tests (in `crates/deadreckon-providers/tests/registry.rs`):
- `registry_builtin_lists_six_existing_providers`
- `descriptor_toml_round_trips_serde`
- `provider_overrides_d_files_load_in_lexical_order`
- `override_file_can_extend_built_in_default_binary`
- `override_file_can_register_brand_new_id`
- `parse_custom_command_handles_quoted_paths`
- `parse_custom_command_handles_escaped_chars`

### P2 — Migrate existing providers (no behavior change)

- Existing `CliClaudeCodeProvider`, `CliCodexProvider`, HTTP
  `ProviderAdapter`, `ScriptedSmokeProvider` keep their construction
  signatures; `ProviderRouter::build` now consults
  `ProviderRegistry::builtin().get(<id>)` for descriptor data
  (binary, sandbox allowlists, model catalog) instead of inline
  constants.
- All existing tests pass unchanged.
- The `ProviderKind` enum gains a `Generic(String)` variant for the
  new descriptor-driven path; old variants remain.

Depth tests:
- `migrated_cli_codex_uses_descriptor_default_binary`
- `migrated_cli_codex_uses_descriptor_sandbox_writes`
- `migrated_anthropic_uses_descriptor_default_endpoint`
- `existing_provider_router_tests_pass_post_migration`
- `provider_kind_generic_variant_round_trips_serde`

### P3 — `deadreckon detect` verb + actionable errors

- Implement `detect` per the rider: probe each registered provider
  (CLI: `LookPath` + `version_probe`; HTTP: env var presence;
  LocalHttp: `--ping` only).
- Plain output is one line per provider; `--json` per the schema.
- Per-provider `try:` lines come from `install_hint`.
- Probe errors classify into `not_found`, `permission_denied`,
  `version_mismatch`, `auth_missing`, `endpoint_unreachable`.

Depth tests (in `crates/deadreckon/tests/detect.rs`):
- `detect_lists_every_registered_provider`
- `detect_marks_cli_codex_ok_when_fake_binary_in_path`
- `detect_marks_anthropic_missing_credential_when_env_unset`
- `detect_marks_cli_provider_version_mismatch_with_min_known_good`
- `detect_json_output_matches_schema`
- `detect_ping_flag_required_for_http_endpoint_probe`

### P4 — `deadreckon providers list` verb + `--models`

- New verb backed by the registry; default output is the configured
  providers in current scope; `--all` widens to every built-in.
- `--models` adds a per-provider model catalog block.
- `--full` emits exact IDs/paths for scripts.

Depth tests:
- `providers_list_default_shows_configured_only`
- `providers_list_all_includes_built_ins_not_in_config`
- `providers_list_models_includes_aliases`
- `providers_list_full_emits_exact_paths_no_truncation`

### P5 — `cli:cursor-agent`

- Author `descriptors/cli-cursor-agent.toml` per §"CLI provider
  exec templates".
- Fake `cursor-agent` binary in `tests/bin/cursor-agent.sh` for
  cargo tests.
- The descriptor-driven path runs through the existing CLI
  subprocess machinery (`cli_common::run_cli`); no new module
  needed beyond the descriptor.

Depth tests:
- `cli_cursor_agent_descriptor_loads_from_built_ins`
- `cli_cursor_agent_invokes_print_flag_with_prompt`
- `cli_cursor_agent_sandbox_writes_include_dot_cursor`
- `cli_cursor_agent_detect_finds_fake_binary_via_path`
- `cli_cursor_agent_install_hint_links_to_cursor_com_cli`

### P6 — `cli:gemini`

Same shape as P5 with `descriptors/cli-gemini.toml`.

Depth tests:
- `cli_gemini_descriptor_loads_from_built_ins`
- `cli_gemini_invokes_prompt_flag`
- `cli_gemini_install_hint_links_to_ai_google_dev_get_started`
- `cli_gemini_detect_finds_fake_binary_via_path`

### P7 — `cli:aider`

Same shape as P5 with `descriptors/cli-aider.toml`. Note `--yes`
and `--no-auto-commits` are required for unattended use (aider's
default is interactive).

Depth tests:
- `cli_aider_descriptor_loads_from_built_ins`
- `cli_aider_invokes_no_auto_commits_yes_message_flags`
- `cli_aider_install_hint_links_to_aider_chat_install`
- `cli_aider_detect_finds_fake_binary_via_path`

### P8 — `gemini` HTTP (Google AI Studio)

- New module `crates/deadreckon-providers/src/http_gemini.rs`
  implementing the `Provider` trait against
  `/v1beta/models/{model}:generateContent`.
- Header `x-goog-api-key: <env var GEMINI_API_KEY>`.
- Response parser extracts `candidates[0].content.parts[0].text` +
  `usageMetadata.{promptTokenCount,candidatesTokenCount}`.
- Pricing defaults from `descriptors/gemini.toml` (Gemini 1.5 Pro
  $1.25/$5 per million; Gemini 2.0 Flash $0.075/$0.30 per million —
  values as published; commit notes the publication date).

Depth tests (mock HTTP server in `tests/mock_server_gemini.rs`):
- `gemini_http_request_uses_x_goog_api_key_header`
- `gemini_http_request_path_substitutes_model`
- `gemini_http_response_parses_candidates_text`
- `gemini_http_spend_uses_descriptor_pricing`
- `gemini_http_refuses_when_env_var_unset_with_try_hint`

### P9 — `ollama` LocalHttp (no key)

- New module `crates/deadreckon-providers/src/http_ollama.rs` against
  `/api/chat`.
- `has_credential()` GETs `/api/tags` with a 2 s timeout; success
  → credentialed.
- No spend (`cost_usd: 0.0`, `subscription: false`,
  `wall_time_seconds: Some(elapsed)`).
- Default endpoint `http://localhost:11434` overridable via
  descriptor or `OLLAMA_HOST` env.

Depth tests:
- `ollama_credential_check_pings_api_tags`
- `ollama_chat_request_uses_messages_array`
- `ollama_response_parses_message_content`
- `ollama_spend_records_wall_time_no_dollars`
- `ollama_endpoint_override_via_env_var`

### P10 — Model catalog + `--model` resolution

- Implement the `resolve_model_to_provider` algorithm in
  `crates/deadreckon-providers/src/model_resolve.rs`.
- `deadreckon run "..." --model <id>` consults the resolver before
  building the router; the resolved descriptor wins over
  `default_provider` for that run.
- Aliases supported (e.g., `--model opus` → `claude-opus-4-1`).
- Tiebreak: `default_provider`, then `fallback` order, then
  alphabetical descriptor id.
- Nearest-model suggestion uses Levenshtein over catalog ids +
  aliases; cap suggestions to top 3.

Depth tests:
- `model_resolves_unique_to_one_provider`
- `model_resolves_via_alias`
- `model_resolution_ambiguous_lists_all_candidates`
- `model_resolution_unknown_suggests_nearest_three`
- `model_resolution_no_credentialed_candidate_includes_install_hint`
- `model_tiebreak_honors_default_provider`

### P11 — Friendliness pass + AS-BUILT + CHANGELOG

- **`deadreckon init`**: after detection, populate `fallback` chain
  with currently-credentialed descriptors in this priority:
  CLI subscriptions first (free), then HTTP keys, then local
  (Ollama). Preview the proposed config before write.
- **`deadreckon doctor`** (extends audit-harden rider): the providers
  block iterates the registry, not the hardcoded list.
- **`deadreckon --help`**: `Run` group's `--provider` and `--model`
  flag descriptions reference `deadreckon providers list`.
- **AS-BUILT update** (`/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`):
  - Add `## 27. Provider Registry` with subsections:
    27.1 Descriptor schema and override files
    27.2 Built-in descriptor inventory
    27.3 `detect` and `providers list` verbs
    27.4 Model catalog and `--model` resolution
    27.5 Adding a new provider (one-page recipe)
  - Update §10 Provider Model: replace the hard-coded six-kinds
    enumeration with a pointer to §27.
  - Update §19 Configuration & BYOK: note the
    `~/.deadreckon/providers.d/` override mechanism.
  - Update §22 (Built vs Scaffolding-Thin):
    - **Add to "Built and reliable"**: provider registry,
      `cli:cursor-agent`, `cli:gemini`, `cli:aider`, `gemini` HTTP,
      `ollama` LocalHttp, `--model` resolver.
    - This rider does not close any prior §22 thin items; it adds
      capability.
- **CHANGELOG append**:

  ```
  ## Provider registry (alpha) — 2026-05-11

  - Provider layer is now descriptor-driven. Built-in descriptors compile in via include_str!; user overrides at ~/.deadreckon/providers.d/<id>.toml.
  - Migrated anthropic, openai, openai-compatible, smoke, cli:claude-code, cli:codex with no behavior change.
  - New CLI providers: cli:cursor-agent, cli:gemini, cli:aider (each ships with a fake-binary depth test).
  - New HTTP providers: gemini (Google AI Studio /v1beta/models/...:generateContent) and ollama (local /api/chat, no key).
  - New verbs: deadreckon detect [<id>] (registry-driven probe with actionable try: hints) and deadreckon providers list [--models|--all|--full].
  - --model <id> resolves through the registry with alias support; ambiguous models list candidates; unknown models suggest the nearest three.
  - deadreckon init populates the fallback chain from detected providers (subscriptions first, then HTTP keys, then local).
  ```

(P11 is doc-only; no depth test.)

## Integration matrix

| Surface | What changes |
|---|---|
| `crates/deadreckon-providers/src/lib.rs` | New `ProviderKind::Generic` variant; `ProviderRouter::build` consults registry |
| `crates/deadreckon-providers/src/registry/` | New module |
| `crates/deadreckon-providers/descriptors/*.toml` | Six migrated + five new descriptor files |
| `crates/deadreckon/src/main.rs` | New verbs `detect`, `providers list`; `--model` flag wired to resolver |
| `~/.deadreckon/providers.d/*.toml` | New runtime location for overrides (absent = built-ins only) |
| `~/.deadreckon/config.toml` | Existing keys unchanged; `default_provider` and `fallback` resolve via registry |
| `deadreckon doctor` (audit-harden rider) | Iterates registry instead of hardcoded list |
| Frontmatter / `Doc-writer:` line | Continues to name the run provider; no schema change |
| TUI `attach` | No change |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| `<id>: not on PATH` | (descriptor `install_hint.try_lines` joined with `\n`) |
| `<id>: version <X> below known-good <Y>` | (descriptor `install_hint.try_lines` upgrade form) |
| `<id>: auth failed (<env_var> not set)` | `export <env_var>=...` (from `auth.env_var`) |
| `<id>: endpoint <url> timed out` | `deadreckon detect <id> --ping --timeout 10` |
| `unknown model '<m>'; nearest: <a>, <b>` | `deadreckon providers list --models` |
| `model '<m>' available via [<a>, <b>]; ambiguous` | `--provider <a>` or `deadreckon config set default_provider <a>` |
| `model '<m>' known but no credentialed provider` | (joined `install_hint.try_lines` from candidate descriptors) |
| `no provider '<id>' in registry` | `deadreckon providers list --all` |
| `provider override file <path> malformed at line N` | (descriptor field name + valid example) |

(Each parameterized over a depth test; see P3/P10/P11.)

## Config additions (`config.toml`)

```toml
[defaults]
# Existing keys (provider, sandbox, max_spend, ...) unchanged.
model = "claude-sonnet-4-5"        # NEW: --model default; resolves via registry

[detect]
ping_timeout_seconds = 10          # detect --ping cap
http_pricing_warn_age_days = 90    # warn when descriptor pricing wasn't reviewed in N days
```

## Out of scope (explicitly not in this milestone — V1 candidates)

- **Bedrock / Vertex** — signed AWS / Google IAM auth shapes. Both
  warrant their own descriptor-extension phase; deferred.
- **MCP-server providers.** A descriptor `kind = Mcp` with stdio
  spawn + JSON-RPC. Architecturally clean but its own rider's worth
  of work.
- **Provider-side hooks / middleware.** Pre-request / post-response
  rewrite (e.g., redact secrets, log to a sink). V1.
- **Streaming responses.** Current adapters are non-streaming; the
  registry doesn't preclude streaming but adding it is V1.
- **Cost-aware routing.** "Pick the cheapest credentialed descriptor
  whose model satisfies the request" — needs a cost-vs-quality
  heuristic; V1.
- **Provider-specific tool schemas.** Each provider's tool-call shape
  (Anthropic vs OpenAI vs Gemini) is normalized in the turn loop
  today; richer per-provider tool descriptors are V1.
- **Auto-PR of new descriptors back upstream.** Users add a TOML;
  contributing it back is manual.
- **`cli:opencode`, `cli:goose`, `cli:amp`, `cli:q`, `cli:crush`** —
  shipping descriptors plus tests for each is its own follow-up
  rider; this rider lands the mechanism + 3 representative new CLIs.
- **Curated descriptors for `together`, `groq`, `openrouter`,
  `mistral`, `grok`** as first-class IDs. They work today via
  `openai-compatible`; first-class IDs are a V1 polish.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (utility, free):
- `which` — already in workspace; used for binary detection.
- `toml` — already in workspace; used for descriptor parsing.
- `strsim` (or hand-rolled Levenshtein) — for nearest-model
  suggestion. Hand-rolled if we want zero new deps; `strsim` is
  ~200 LoC of pure Rust.

Tier 2 (architectural, log to `DEPENDENCIES.md`): none expected.

Tier 3 (blocked): same blocks as prior riders.

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes.** Descriptor state in files.
- **One depth test before each phase implementation.** A phase whose
  tests were never red is suspect.
- **Backwards-compatible.** Existing `config.toml` files work
  unchanged; existing tests pass without modification; the legacy
  `ProviderKind` variants remain.
- **Descriptors are TOML, not Rust literals.** The source of truth
  for built-ins is `descriptors/*.toml`; depth tests parse the TOML
  rather than asserting on Rust constants.
- **Per-provider error messages mirror specstory-cli's actionability.**
  Every probe failure lands a `try:` line drawn from the descriptor's
  `install_hint`. Depth-tested per provider.
- **No `Project files`-style generic fallbacks.** A descriptor either
  declares precisely what it is (binary, endpoint, auth, model
  catalog) or it doesn't get registered. No "unknown HTTP provider"
  catch-all.
- **No silent expansion.** Anything beyond P1–P11 goes into
  `V1-CANDIDATES.md`.
- **Spec-pinning invariants.** Descriptor TOML shape, the `detect`
  output format (both plain and `--json`), the `--model` resolution
  algorithm, and the error-footer text are depth-tested; changing
  whitespace or ordering changes the spec.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with the relevant depth tests passing and a
  CHANGELOG entry naming the SHA.
- After P11, capture an asciinema cast at
  `/Users/gdc/deadreckon/demo-provider-registry.cast` showing
  `deadreckon detect`, `providers list --models`, and a
  `run --model gemini-1.5-pro` smoke against a mock HTTP server.
- If a phase reveals a V1-architecture decision, stop and log it in
  `V1-CANDIDATES.md`; do not silently expand scope.
- Pricing values pulled from each provider's published pricing page
  on the commit date — note the date in the descriptor TOML
  comment; future updates land in a follow-up rider.
