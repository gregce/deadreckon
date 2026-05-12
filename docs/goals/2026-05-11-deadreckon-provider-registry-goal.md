GOAL: Make deadreckon's provider layer pluggable, mining `/Users/gdc/specstory-cli`'s SPI pattern. Today's six providers are hardcoded — `ProviderKind` enum + per-CLI module + router match arm — so adding a new CLI (`cursor-agent`, `gemini`, `aider`) or HTTP route (Google Gemini, Ollama) is ~125 LoC of boilerplate every time. specstory-cli ships claude/codex/cursor/gemini behind one `pkg/spi/factory/registry.go` with one-line additions, a uniform `Provider.Check(customCommand)` interface, and per-provider actionable error messages. Direct-model driving is largely covered by `openai-compatible` today; concrete gaps are Gemini API (different request shape) and Ollama (local, no key). Headline word: **Pluggable**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` — substrate; especially §10 / §19.
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-deadreckon-provider-registry-rider.md` — descriptor schema, registry, depth tests.
- `/Users/gdc/deadreckon/docs/goals/2026-05-10-deadreckon-build-rider.md` — predecessor; provider invariants hold.
- specstory-cli exemplars: `/Users/gdc/specstory-cli/pkg/spi/{provider.go,cmdline.go,factory/registry.go}` and `/Users/gdc/specstory-cli/pkg/providers/{claudecode,codexcli,cursorcli,gemini}/provider.go`.
- Current Rust seams: `/Users/gdc/deadreckon/crates/deadreckon-providers/src/{lib.rs,cli_claude_code.rs,cli_codex.rs,cli_common.rs}`.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` — invariants hold.

**Posture.** Stays `alpha`. No `PipelineState` schema changes. No `git push`. Edits inside `/Users/gdc/deadreckon/`. V1 decisions → `docs/V1-CANDIDATES.md`.

**Mechanism (full schema in rider).**

- `ProviderDescriptor` declares everything to introspect, probe, invoke, and meter a provider: `id`, `kind`, default binary/endpoint, `version_probe`, `exec_template`, sandbox allowlists, `model_catalog`, `fs_detection_paths`, `install_hint`.
- `ProviderRegistry` reads built-in descriptors compiled into the binary plus user overrides at `~/.deadreckon/providers.d/<id>.toml` — single edit point.
- `ProviderKind` gains a `Generic` variant for descriptor-driven dispatch; old variants stay for back-compat.

**Providers in scope.**

- Migrated, no behavior change: `anthropic`, `openai`, `openai-compatible`, `smoke`, `cli:claude-code`, `cli:codex`.
- New CLI: **`cli:cursor-agent`**, **`cli:gemini`**, **`cli:aider`**.
- New HTTP: **`gemini`** (Google AI Studio), **`ollama`** (local, no key).
- Model catalog with `--model <id>` resolution; auto-routes to a credentialed descriptor that lists the model.

**New verbs.**

- `deadreckon detect [<id>]` — registry-driven probe; actionable error per failure.
- `deadreckon providers list` — registered providers with PATH / credential / version / sub-vs-metered status.

**Friendliness.** Auto-detect at `init`; preview before writing config; refuse with `try:` from descriptor `install_hint`; rollback via `deadreckon config restore`; lifecycle hints after every action.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check` green → conventional-commit → CHANGELOG. P11 adds §27 "Provider Registry" to AS-BUILT and updates §10 + §19.

**Verification.**

- Commands green every commit; every rider depth test present and passing.
- Detect smoke: `deadreckon detect` lists every registered provider; `cli:cursor-agent` (fake binary in tests) probes green; `gemini` HTTP refuses without `GEMINI_API_KEY` and includes `try:`.
- Model-catalog smoke: `--model gpt-4o-mini` selects `openai`; `--model gemini-1.5-pro` selects `gemini` (or `cli:gemini` if no API key).
- No edits outside `/Users/gdc/deadreckon/`. No `git push`. No `PipelineState` schema changes.

**Stop when** verification passes, AS-BUILT updated, CHANGELOG has a "Provider registry (alpha)" section, committed locally.
