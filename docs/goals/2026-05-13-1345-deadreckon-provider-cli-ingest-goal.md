GOAL: Make deadreckon's CLI-provider path extensible from descriptor to TUI activity feed, using `/Users/gdc/agentsview` as parser/discovery research without a runtime dependency. Today `dr detect` and descriptors are ahead, but launch still routes through two concrete CLI structs and attach only knows `ProviderJsonlSchema::{CodexCli, ClaudeCode}`. Land the missing bridge: descriptor `[ingest]`, descriptor-driven discovery/cwd matching, schema-keyed row parsers, generic `exec_template` CLI launch, canonical tool categories, and two pilots (`cli:gemini`, `cli:opencode` file mode). Headline word: **Ingestible**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - provider/TUI substrate.
- `/Users/gdc/deadreckon/docs/design/PROVIDER-CLI-INGEST.md` - research matrix.
- `/Users/gdc/deadreckon/docs/goals/2026-05-13-1345-deadreckon-provider-cli-ingest-rider.md` - schemas, focused verification, parser scope.
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-2248-deadreckon-provider-registry-rider.md` - registry invariants.
- `/Users/gdc/agentsview/internal/parser/{types.go,discovery.go,taxonomy.go,gemini.go,opencode.go}` - prior art; port ideas, do not link.
- Current seams: provider `registry`, `router.rs`, `cli_common.rs`, CLI adapters, and `crates/deadreckon/src/main.rs`.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` - invariants hold.

**Posture.** Stays `alpha`. No `PipelineState` schema changes and no provider-owned log rewrites. Descriptor schema changes are allowed. No `git push`. Edits inside `/Users/gdc/deadreckon/`. Major architectural decisions or SQLite-backed ingest scope go to `docs/V1-CANDIDATES.md`.

**Deliverables.**

- `ProviderDescriptor` gains optional `[ingest]` metadata: env override, default dirs, watch hints, schema string, cwd-match strategy, file glob, freshness, and id prefix.
- Codex and Claude descriptors backfill `[ingest]`; existing TUI lines and context telemetry stay behavior-compatible.
- Attach provider activity resolves through descriptors and a schema dispatch table, not hard-coded provider IDs.
- `deadreckon-providers` gets a canonical tool taxonomy ported from agentsview.
- Generic CLI provider renders descriptor `exec_template`, preserving Codex's trailing `--` prompt delimiter and sandbox placeholder behavior.
- `cli:gemini` and `cli:opencode` descriptors, probes, sandbox writes, and TUI ingest fixtures ship. OpenCode SQLite is deferred unless dependency-light.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implement -> run the rider's focused verification for touched crates only -> conventional-commit -> CHANGELOG. P11 updates AS-BUILT provider/TUI sections and the built-vs-thin accounting.

**Verification.**

- Do **not** run `make verify`, release builds, smoke, stress, or full-workspace tests by default. Use the rider's targeted commands per phase.
- Every rider depth test present and passing; `cargo fmt --check` and clippy only for touched crates are green.
- Compatibility smoke: existing Codex and Claude provider-activity tests still emit the same activity semantics (`agent`, `thinking`, `tool`, `result`, `todo`, `tokens`) and context counts.
- Generic CLI smoke: a fake `cli:local-test` descriptor from `providers.d` routes through `exec_template`, writes output, reports subscription wall time, and needs no new `ProviderKind` variant.
- Pilot ingest smoke: Gemini JSON/JSONL and OpenCode file-mode fixtures produce TUI activity lines and context telemetry without touching real `~/.gemini` or `~/.local/share/opencode`.
- No edits outside `/Users/gdc/deadreckon/`. No `git push`. No provider session truncation or undo integration.

**Stop when** verification passes, AS-BUILT and CHANGELOG describe "Provider CLI ingest (alpha)", `docs/V1-CANDIDATES.md` captures deferred SQLite/undo/bulk-agent scope, and the work is committed locally.
