# Provider CLI Detection, Registration, and Log Ingest

**Status:** Design  
**Last updated:** 2026-05-13  
**Companion reading:** `docs/AS-BUILT-ARCHITECTURE.md` §22 (providers), §29 (workspace hygiene); `docs/goals/*provider-registry*`

## TL;DR

Deadreckon ships descriptor-backed support for two CLI agent providers today
(`cli:claude-code`, `cli:codex`). The recent commits `6bdafe2 → fe0297a → 1af0778`
moved provider identity, sandbox paths, model catalogs, and `dr detect` probing
into TOML descriptors. **Detection, launch, and sandboxing are now generalized.
Log ingest is not** — the TUI activity feed and the per-provider parsers are
still a hand-coded two-arm match on `ProviderJsonlSchema { CodexCli, ClaudeCode }`.

Meanwhile, `agentsview` already has a parser registry covering **24 agents** with
a uniform `AgentDef` shape, a canonical tool-category taxonomy, and per-agent
session-discovery functions. This doc maps what we'd need to extend the
descriptor schema and refactor the JSONL pipeline so that adding a third CLI
provider (e.g. `cli:gemini`) requires **one TOML file plus one parser variant**
rather than ~15 new match arms.

## 1. Two registries, one identity layer

Deadreckon and agentsview already converge on most of the same identity fields.
They diverge after that because they have different jobs.

| Concern | deadreckon (this repo) | agentsview |
|---|---|---|
| Identify the agent | `ProviderDescriptor.id` | `AgentDef.Type` |
| Display name | `display_name` | `DisplayName` |
| Default state dirs | `fs_detection_paths` | `DefaultDirs` |
| Env var override | — *(missing)* | `EnvVar` |
| ID prefix for namespacing | — *(missing)* | `IDPrefix` |
| Watched subdirs | — *(missing)* | `WatchSubdirs`, `ShallowWatch` |
| Launch (binary, args, model) | `default_binary`, `exec_template`, `model_arg` | n/a |
| Auth + sandbox + version probe | `auth`, `sandbox_writes`, `version_probe` | n/a |
| Install hint | `install_hint`, `try_lines` | n/a |
| Session-file discovery | hard-coded for codex/claude in `main.rs` | `DiscoverFunc` / `FindSourceFunc` |
| Parse JSONL → activity lines | hard-coded `codex_activity_line` / `claude_activity_lines` | per-agent parser + canonical `ParsedSession` / `ParsedMessage` |
| Tool name normalization | — | `NormalizeToolCategory` (single switch, 100+ raw names → 9 categories) |

The two registries are **complementary, not redundant**. Treat agentsview as
prior art for the *post-run* side of the descriptor (discover, parse, classify)
and pull only the structural shape — deadreckon should not depend on agentsview
as a library.

## 2. End-to-end lifecycle of `cli:codex` today

This is the canonical example. Every CLI provider we add will travel the same
six phases.

### Phase 1 — Descriptor

```toml
# crates/deadreckon-providers/descriptors/cli-codex.toml
id              = "cli:codex"
kind            = "cli"
default_binary  = "codex"
sandbox_writes  = ["~/.codex"]
fs_detection_paths = ["~/.codex/sessions"]
subscription    = true

[auth]           kind = "subscription"
[version_probe]  args = ["--version"]
[exec_template]  args_template = ["--ask-for-approval", "never", "exec", ...]
                 model_arg = "--model"
                 timeout_seconds = 1800
```

`include_str!`-embedded at compile time and registered through
`ProviderRegistry::builtin()` (`crates/deadreckon-providers/src/registry/mod.rs:195`),
then overlaid with `$DEADRECKON_HOME/providers.d/*.toml` via `with_overrides()`
(line 206).

### Phase 2 — Detection (`dr detect`)

Wired in commit `1af0778`. Entry point `detect_command` in
`crates/deadreckon/src/main.rs:1011`. For each descriptor of kind `cli`,
`probe_cli_descriptor` (`registry/mod.rs:321`) does:

1. Resolve binary on PATH via `which::which` (line 333).
2. If `version_probe.args` is set, run `{binary} --version`, validate against
   `expect_substring` and `min_known_good` (lines 344–390).
3. Return a `ProviderProbeResult { status, location, version, error_kind,
   try_lines, fs_artifacts }`.

This is already descriptor-driven and generalizes for free.

### Phase 3 — Launch

`CliCodexProvider::run` (`crates/deadreckon-providers/src/cli_codex.rs:35`)
builds the arg vector from the descriptor's `exec_template`, then delegates to
`run_cli` (`cli_common.rs:23`) which either spawns directly via
`tokio::process::Command` or routes through `deadreckon_sandbox::run` with a
`ToolSandboxPolicy::cli_provider`. The sandbox read/write allowlists are built
by `cli_provider_write_allowlist` (line 161) which already consults the
descriptor's `sandbox_writes` — the legacy `if provider.contains("codex")`
fallback at line 178 should be deleted once all providers ship descriptors.

### Phase 4 — JSONL ingest for the TUI (THE BOTTLENECK)

Driven from `crates/deadreckon/src/main.rs`. Two hard-coded sites:

```rust
// main.rs:10861 — provider_jsonl_log_spec
match state.provider.as_deref()? {
    "cli:codex"       => Some(ProviderJsonlLogSpec {
        schema: ProviderJsonlSchema::CodexCli,
        roots:  vec![home.join(".codex/sessions")],
        since:  state.started_at - ChronoDuration::minutes(2),
    }),
    "cli:claude-code" => { /* per-workdir project dirs */ }
    _ => None,
}
```

```rust
// main.rs:10936 — provider_jsonl_activity_lines
match schema {
    ProviderJsonlSchema::CodexCli   => codex_activity_line(line, activity).into_iter().collect(),
    ProviderJsonlSchema::ClaudeCode => claude_activity_lines(line, activity),
}
```

`codex_activity_line` (`main.rs:11071`) is a JSON match on
`(value.type, payload.type)`:

| `(type, payload.type)` | Emitted line | Side effects |
|---|---|---|
| `(event_msg, task_started)` | `codex started` | — |
| `(event_msg, agent_message)` | `agent <message>` | — |
| `(event_msg, token_count)` | `tokens N/W rate P%` | sets `context_tokens`, `context_window` |
| `(response_item, function_call)` | `tool <name> <summary>` | — |
| `(response_item, function_call_output)` | `result <output>` | — |

`claude_activity_lines` dispatches on `type ∈ {assistant, user, attachment}` and
walks `message.content[]` extracting text / thinking / tool_use / tool_result
blocks plus `message.usage`. Session ↔ working-dir matching diverges:

- **Codex**: scan first 8 lines for `type == "session_meta"`, read `payload.cwd`
  (`jsonl_session_meta_cwd_matches`, `main.rs:10975`).
- **Claude**: scan first 80 lines for any record with top-level `cwd`
  (`jsonl_top_level_cwd_matches`, `main.rs:10999`).

After parsing, `cap_provider_activity` (line 10962) keeps the last 240 lines
(reverse → take(240) → reverse) to bound TUI memory. Lines are free-form
strings; the TUI consumes them at `attach_activity_lines` (`main.rs:11764`) and
mixes them with deadreckon's own spend/traces/events feed.

### Phase 5 — Undo

`undo_command` (`main.rs:9240`) restores **filesystem snapshots only**
(`crates/deadreckon-core/src/artifacts.rs:82`, `restore_snapshot`).
`truncate_run_artifacts_after_turn` (`crates/deadreckon-runtime/src/turn_loop.rs:1100`)
trims `traces.jsonl` and `spend.jsonl` to entries with `turn <= from_turn`.

**Provider JSONL files are never touched.** They live in `~/.codex/sessions/`
and `~/.claude/projects/`, deadreckon does not own them, and the provider's own
session UUIDs do not map to deadreckon turn IDs in any persisted index.

If we ever want undo to also rewind the provider session (e.g. so a re-run from
turn 3 doesn't see the cancelled turn 4 in the provider's own context), we'd
need:

- A turn→`(session_path, byte_offset)` index, persisted somewhere like
  `provenance.jsonl`, populated by the ingest loop as it tails each provider
  JSONL.
- A `truncate_provider_session_after_turn(provider, turn)` analogous to
  `truncate_jsonl_after_turn` (`turn_loop.rs:1123`) that respects the per-agent
  framing (some agents append, some rewrite, some are sqlite-backed).

This is a non-trivial seam and is **not** something the descriptor refactor
alone unlocks — it requires a turn-keyed offset log. Out of scope for the first
ingest refactor; called out here so we don't paint ourselves into a corner.

### Phase 6 — Inventory of hard-coded codex sites

Every place we currently key on the literal `"cli:codex"`, the enum
`ProviderJsonlSchema::CodexCli`, or the path fragment `.codex`. Each is a
generalization blocker.

| File | Line(s) | What it does | Make descriptor-driven |
|---|---|---|---|
| `deadreckon-providers/src/types.rs` | 18–26, 62 | `enum ProviderKind { CliCodex, ... }` | Either replace with `Generic(String)` or auto-derive variants from descriptor IDs |
| `deadreckon-providers/src/config.rs` | 133 | `"cli:codex"\|"cli-codex" => ProviderKind::CliCodex` | Drop — resolve via registry |
| `deadreckon-providers/src/router.rs` | 155, 171 | `ProviderKind::CliCodex => Box::new(CliCodexProvider::new(...))` | Trait-object factory keyed on descriptor `kind`+`id` |
| `deadreckon-providers/src/http.rs` | 65, 81, 103, 159, 223, 235 | Auth/metering/rate-limit shaping | Move to descriptor `RequestShape` |
| `deadreckon-providers/src/cli_common.rs` | 178 | `if provider.contains("codex")` fallback | Delete; descriptor `sandbox_writes` already covers it |
| `deadreckon/src/main.rs` | 827 | `command_exists("codex") => "cli:codex"` auto-suggest | Scan registry for `kind=cli` descriptors with matching `default_binary` |
| `deadreckon-runtime/src/turn_loop.rs` | 1230 | `"cli:codex" => "codex.out"` filename | Pull from descriptor (new field) |
| `deadreckon/src/main.rs` | 10843–10845 | `enum ProviderJsonlSchema { CodexCli, ClaudeCode }` | See §3 below — descriptor `[ingest].schema` string + dispatch table |
| `deadreckon/src/main.rs` | 10865–10884 | `provider_jsonl_log_spec` hard-coded roots | Descriptor `[ingest].roots` |
| `deadreckon/src/main.rs` | 10931 | `CodexCli => session_meta_cwd_matches` | Descriptor `[ingest].cwd_match` enum |
| `deadreckon/src/main.rs` | 10942 | `CodexCli => codex_activity_line` | Parser-variant trait or function pointer table |
| `deadreckon/src/main.rs` | 5862, 6237, 6520, 6649, 6637 | Provider presets + fallback chain | Descriptor `[defaults]` + `[fallback]` |

The deepest blocker is the `ProviderKind` enum: it's referenced in ~12 match
arms and forces every new provider to touch core types. That's a separate
refactor from the ingest work; it can land independently.

## 3. Proposed descriptor extension — an `[ingest]` block

The current descriptor only describes the *launch* side of an agent. To make
log ingest table-driven, add an optional `[ingest]` block. Anything without
`[ingest]` keeps today's behavior (no TUI activity feed).

```toml
# Example for codex:
[ingest]
id_prefix      = "codex:"                       # for future cross-agent UUIDs
env_var        = "CODEX_SESSIONS_DIR"           # override for default_dirs
default_dirs   = ["~/.codex/sessions",
                  "~/.codex/archived_sessions"]
watch_subdirs  = []                             # nested watch hints
shallow_watch  = false
schema         = "codex-cli"                    # picks parser variant
cwd_match      = "session-meta"                 # vs "top-level" | "directory-field"
cwd_match_path = "payload.cwd"                  # JSON pointer; defaults per cwd_match
session_id_from = "filename-uuid"               # vs "field:sessionId" | "filename-stem"
file_glob      = "*.jsonl"                      # file_based agents only
sqlite         = false                          # opencode dual-mode opt-in
```

Two things this **does not** try to do:

1. **Express the activity-line parser declaratively.** That belongs in Rust —
   each `schema` value maps to a function pointer in a small dispatch table.
   Trying to make the parser table-driven (e.g. via JSON pointers and label
   templates) buys little: the codex extractor is ~50 lines and gets compiled
   sanity-checked; a config-driven equivalent would be longer and harder to
   debug.

2. **Express the full agentsview-level parser** (ParsedSession / ParsedMessage
   / token aggregation / fork detection). The TUI feed only needs activity
   lines + running context-token counts. The richer parser is a future
   investment and should land behind a feature flag once we have a concrete
   consumer (analytics, undo by-turn, replay).

### Sister concern: a ported `normalize_tool_category`

Lift agentsview's `internal/parser/taxonomy.go:8–206` verbatim into
`deadreckon-providers` as `pub fn normalize_tool_category(raw: &str) ->
ToolCategory`. It's pure data, has zero runtime dependencies, and every future
"what is this agent doing" surface will need it. The Go switch is the spec —
translate it as a Rust `match` and pin it with a unit-test table.

## 4. Per-agent extraction matrix

agentsview already parses 24 agents. Most are IDE plugins (VSCode-Copilot,
Cursor, Kiro IDE, Positron) or hosted SaaS (Claude.ai, ChatGPT, Warp, Piebald,
Forge) and have no launchable binary — skip them. The CLI subset deadreckon
could plausibly launch:

| Agent | Default binary | Sessions on disk | `cwd_match` style | Storage shape | Quirks | Priority |
|---|---|---|---|---|---|---|
| **gemini** | `gemini` | `~/.gemini/tmp/` | n/a (env-derived project) | JSON or JSONL (dual) | Thought blocks; results inline in tool_call | **high** |
| **opencode** | `opencode` | `~/.local/share/opencode/storage/session/<proj>/` *or* `~/.local/share/opencode/opencode.db` | `directory` field | JSON file *or* SQLite (auto-detect via `ResolveOpenCodeSource`) | Dual-mode storage; lazy subdirs; SQLite virtual paths | **high** |
| **amp** | `amp` | `~/.local/share/amp/threads/T-*.json` | `env.initial.trees[0].displayName` | JSON (one file per thread) | Traces under `meta.traces[]`; epoch-ms timestamps | medium |
| **openhands** | `openhands` | `~/.openhands/conversations/<uuid>/` | session metadata file | Directory-per-session | Multiple event files inside a session dir; shallow-watch root | medium |
| **iflow** | `iflow` | `~/.iflow/projects/session-*.jsonl` | top-level `cwd` | JSONL | Claude-shaped content blocks, sliding-window uuid DAG (not forks) | medium |
| **copilot** | `gh copilot` / `copilot` | `~/.copilot/session-state/*.jsonl` | `session.start.data.context.cwd` | JSONL (event stream) | Event-based, not message-centric | medium |
| **cortex** | `cortex` | `~/.snowflake/cortex/conversations/` | tbd | JSONL | Snowflake-hosted | low |
| **hermes** | `hermes` | `~/.hermes/sessions/` | tbd | JSONL | Browser + vision tools dominate the taxonomy | low |
| **iflow-alt: pi/kimi/openclaw/zencoder/kiro** | various | various | various | mixed | Niche or pre-release | low |

Each entry maps to one `crates/deadreckon-providers/descriptors/cli-*.toml`
file plus one new `schema` variant in the ingest dispatch table. Most variants
will reuse the same "extract assistant text / tool_use / tool_result / usage"
shape — the differences live in field names and where `cwd` lives.

Detailed per-agent extraction notes (storage, session-ID derivation,
cwd-matching, per-message field paths, termination signals) are in the
exploration appendix at the end of this document.

## 5. Phased rollout

Ordered to minimize blast radius. Each step is independently revertible.

### Step 1 — Refactor the ingest pipeline (no behavior change)

- Add `[ingest]` block to `ProviderDescriptor`. Make it `Option<_>`.
- Replace `provider_jsonl_log_spec` (`main.rs:10861`) with a registry lookup
  that reads `descriptor.ingest.{roots, schema, cwd_match}`.
- Replace `ProviderJsonlSchema` enum dispatch with a small dispatch table
  keyed by the `schema` string, registering `codex-cli` and `claude-code`
  as the two existing entries.
- Backfill `[ingest]` into `cli-codex.toml` and `cli-claude-code.toml` so the
  matrix is fully descriptor-driven.

Acceptance: existing TUI activity feed for codex and claude is byte-identical
in attached/extended runs. No new providers yet.

### Step 2 — Port the tool taxonomy

- Add `crates/deadreckon-providers/src/taxonomy.rs` with
  `normalize_tool_category`, ported from
  `/Users/gdc/agentsview/internal/parser/taxonomy.go`.
- Table-driven unit tests (one row per `case`).
- Wire it into `codex_activity_line` and `claude_activity_lines` so emitted
  lines render canonical categories ("Bash", "Read", "Edit") rather than raw
  names ("shell_command", "read_file", "apply_patch").

Acceptance: TUI tool-use lines normalize across providers; ports of new
parsers reuse it directly.

### Step 3 — Pilot two new CLI providers: `cli:gemini` and `cli:opencode`

These are the two highest-value CLI agents that aren't yet supported, and
together they exercise the schema variants we expect:

- **gemini** validates the dual JSON/JSONL file shape and a no-`cwd` agent.
- **opencode** validates `sqlite = true` (legacy mode) and per-project
  subdirectory enumeration.

Each lands as: one descriptor TOML + one schema variant in the ingest
dispatch + one parser function + an integration test against a captured
session fixture. Update the `command_exists` auto-suggest at `main.rs:827`
to scan the registry instead of branching on hard-coded names.

Acceptance: `dr detect` lists gemini/opencode; running `dr ... --provider
cli:gemini` produces an activity feed in the TUI; sandbox writes go to
`~/.gemini` / `~/.local/share/opencode`.

### Step 4 — Collapse `ProviderKind` enum

Independent refactor. Replace `enum ProviderKind { CliCodex, CliClaudeCode,
... }` (`crates/deadreckon-providers/src/types.rs:18`) with a
descriptor-keyed `ProviderHandle`. Update router and http call sites. This
unblocks bulk CLI provider additions without touching core types per agent.

### Step 5 — Bulk-add remaining CLI descriptors

amp, openhands, iflow, copilot, then the long tail. Each is a TOML + a small
parser variant. By this point the cost per agent should be ~150 lines + a
fixture.

### Step 6 — Future: provider-aware undo

Only if a real workflow demands it. Build a turn → `(session_path, offset)`
index in `provenance.jsonl` during ingest, then extend
`truncate_run_artifacts_after_turn` (`turn_loop.rs:1100`) to truncate
provider JSONL alongside our own artifacts. Defer until requested.

## 6. Risks and open questions

- **Stale provider session shadows.** Codex and Claude both keep historical
  sessions in the default dirs. Today the ingest filters by `since =
  started_at - 2 min` (`main.rs:10868`). New agents need an analogous
  freshness gate; encode it as `[ingest].freshness_minutes` rather than
  hard-coding.
- **OpenCode SQLite mode.** Adding a sqlite reader to deadreckon-providers
  is a real dependency; the `rusqlite` feature surface and build-time impact
  needs sign-off. Alternative: skip SQLite mode in step 3 and ship file-mode
  only.
- **`ProviderKind` enum migration.** Touches the public-ish surface of
  deadreckon-providers. If we expose it for plugin/extension authors, the
  `Generic(String)` migration is breaking. Confirm with the goal+rider for
  the providers crate before step 4.
- **Activity-line stability.** TUI snapshot tests pin some line strings
  (`main.rs:12658, 12683`). When we normalize tool categories in step 2,
  these strings will change. Plan the test refresh atomically.
- **Subscription detection for new agents.** `cli-codex.toml` and
  `cli-claude-code.toml` both set `subscription = true` because their
  binaries handle their own auth. Gemini and opencode may or may not — get
  ground truth before publishing the descriptors.

## Appendix — Per-agent extraction notes

Citations are into `/Users/gdc/agentsview`. Use these as the spec when
porting parser variants.

**codex** — JSONL at `~/.codex/sessions/`, env `CODEX_SESSIONS_DIR`. Session ID
= filename stem (`codex:` prefix added). `cwd` from `session_meta.payload.cwd`
(also `payload.git.branch` for project refinement). Per-message:
`role`, `payload.content[].text`, `response_item.{name, call_id}`, model from
`turn_context.model`, tokens from `event_msg.token_count.info.last_token_usage`,
ISO8601 `timestamp`. Termination: `task_started` / `task_complete` /
`turn_aborted` (`termination.go:74`). Quirk: subagents via
`spawn_agent`/`wait_agent` (`codex.go:218`).

**claude** — JSONL at `~/.claude/projects/`, env `CLAUDE_PROJECTS_DIR`. Session
ID = filename stem (no prefix). `cwd` top-level on first user entry
(`claude.go:187`). Per-message: `message.content[]` blocks (text, tool_use,
tool_result), `message.usage`, `message.stop_reason`,
`message.model`. Termination: `end_turn` stop reason or orphaned tool_use
(`termination.go:90`). Quirks: DAG fork splitting at >3-user-turn gaps,
assistant-chunk merging by `message.id`, compact-summary boundaries
(`claude.go:510, 682, 1003`).

**gemini** — JSON or JSONL at `~/.gemini/tmp/`. Mandatory top-level
`sessionId`. No `cwd` in transcript — project defaults from env. Per-message:
`type ∈ {user, gemini}`, `content` or block array, `toolCalls[]` with embedded
`result[].functionResponse`, `model`, `tokens.{input, output, cached}`. Quirks:
dual JSON/JSONL parsing in one path; thought blocks captured separately.

**opencode** — JSON files at
`~/.local/share/opencode/storage/session/<project>/*.json` **or** legacy
SQLite at `~/.local/share/opencode/opencode.db` (auto-detect via
`ResolveOpenCodeSource` `discovery.go:75`). Session ID: filename stem (file
mode) or `session.id` column (SQLite). `cwd` from `directory` field
(`opencode.go:292`). Subdirs: `storage/{session,message,part}`. Quirk: hybrid
roots where storage and db coexist; virtual path `dbpath#sessionid` for SQLite
sessions.

**amp** — JSON `T-*.json` files under `~/.local/share/amp/threads/`. Session
ID derived from filename (`ampThreadIDFromPath`). Project from
`env.initial.trees[0].displayName`. Messages under `meta.traces[]` with
type-tagged entries (Message, Action, Observation), epoch-ms `timestamp`,
`endTime` on the closing trace.

**openhands** — Directory-per-session under
`~/.openhands/conversations/<uuid>/`. Multiple event files inside; shallow
watch on root. Events tagged `MessageEvent` / `ActionEvent` /
`ObservationEvent` (`openhands.go:18`).

**iflow** — JSONL `session-*.jsonl` under `~/.iflow/projects/`. Session ID =
filename stem with `iflow:` prefix. Top-level `cwd`. Per-message shape matches
Claude (`message.content[]` blocks). **Important:** uuid/parentUuid chains are
sliding-window snapshots, not conversation forks — parse linearly, do not
apply Claude-style fork detection (`iflow.go:27`).

**copilot** — JSONL under `~/.copilot/session-state/`. Session ID from
`session.start.data.sessionId`. `cwd` from `session.start.data.context.cwd`
plus `context.branch`. Event-based stream: `user.message`, `assistant.message`,
`tool.execution_complete`, `assistant.reasoning`, `session.model_change` —
*not* a message-centric format.

**Shared infrastructure to port** (when we go beyond activity-line ingest):

- `internal/parser/linereader.go` — 64 MB max line buffered scanner with
  graceful oversize skipping and offset-based resume.
- `internal/parser/timestamp.go` — eight ISO8601 variants tried in order
  (`parseTimestamp` line 10).
- `internal/parser/termination.go` — unified `Classify` mapping (Claude
  `end_turn`, codex `task_complete`, orphaned tool detection).
- `internal/parser/taxonomy.go` — `NormalizeToolCategory` (already called
  out in step 2 above).
- `internal/parser/discovery.go` — `uuidRe` (line 20), `isDirOrSymlink`
  (line 29), `ResolveOpenCodeSource` (line 75), `ExtractProjectFromCwd` /
  `NormalizeName` helpers.

---

*Sources: this design synthesizes the recent provider commits
(`6bdafe2`, `fe0297a`, `1af0778`, `4132e48`), a full read of
`crates/deadreckon-providers/src/registry/mod.rs`, the cli:codex launch path
(`crates/deadreckon-providers/src/{cli_codex.rs, cli_common.rs}`), the JSONL
ingest in `crates/deadreckon/src/main.rs:10545–11239`, the undo path
(`crates/deadreckon-core/src/artifacts.rs:66`, `crates/deadreckon/src/main.rs:9240`,
`crates/deadreckon-runtime/src/turn_loop.rs:1100`), and the agentsview parser
registry at `/Users/gdc/agentsview/internal/parser/{types.go, taxonomy.go,
discovery.go, termination.go, codex.go, claude.go, gemini.go, opencode.go,
amp.go, openhands.go, iflow.go, copilot.go}`.*
