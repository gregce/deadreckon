GOAL: Harden `deadreckon import` from broad inventory import into a descriptor-driven transcript import workflow that reuses today's provider ingest work. The original intent was read-only cross-tool state sharing: bring Claude Code, Codex, and Cursor histories into deadreckon's trace/provenance shape so `show` and lifecycle tools can reason about them. The current command still reflects that first pass: source-name branches, broad root scans, raw-ish rows, and shallow provenance extraction. Land the next alpha: import uses provider `[ingest]`, selects concrete sessions, normalizes source events, keeps Cursor, and refuses ambiguous/destructive cases with `try:` lines. Headline word: **Recoverable**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - import, provider activity ingest, built/thin accounting.
- `/Users/gdc/deadreckon/docs/goals/2026-05-13-1839-deadreckon-import-ingest-hardening-rider.md` - UX, data files, phases, tests.
- `/Users/gdc/deadreckon/docs/goals/2026-05-13-1345-deadreckon-provider-cli-ingest-rider.md` - descriptor ingest invariants.
- `/Users/gdc/deadreckon/docs/goals/2026-05-13-1705-deadreckon-copilot-pi-providers-rider.md` - newer provider schemas.
- Current code: `crates/deadreckon/src/{cli.rs,main.rs}`, provider registry/descriptors, import fixtures/goldens.
- Prior import riders: `2026-05-11-1400-deadreckon-robust-rider.md` and `2026-05-11-2110-deadreckon-audit-harden-rider.md`.

**Posture.** Stays `alpha`. No `PipelineState` schema changes and no writes to provider-owned histories. New import state lives as files under the imported run root. Avoid descriptor schema growth unless existing `[ingest]` cannot express the source. Larger parser/analytics choices go to `docs/V1-CANDIDATES.md`. No `git push`. Edits inside `/Users/gdc/deadreckon/`.

**Deliverables.**

- `deadreckon import` accepts descriptor provider IDs and legacy aliases: `codex`, `claude-code`, `cursor`, `cli:codex`, `cli:gemini`, `cli:opencode`, `cli:copilot`, `cli:pi`.
- Import discovery reuses `[ingest]` roots, env overrides, storage, file glob, cwd matching, and schema keys instead of maintaining a second hard-coded CLI source table.
- Default import selects one cwd-matched session when possible; ambiguous, empty, or stale matches print candidates and `try:` commands. Whole-root import requires `--all`.
- Each imported run records an `import.json` manifest with source, schema, session id/path, content hash, row counts, provenance counts, and the command needed to reimport.
- Trace details are stable normalized import events; raw source metadata and hashes are preserved for audit. Provenance understands real provider tool/file fields, not only `path` / `file` / `files`.
- Cursor SQLite import remains supported and covered; OpenCode file mode is in scope, OpenCode SQLite stays V1 unless dependency-free.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implement -> focused verification -> conventional local commit -> CHANGELOG. P11 updates AS-BUILT and V1 candidates.

**Verification.**

- Focused matrix green: import tests, provider JSONL parser tests, registry ingest tests, fmt, and clippy for touched crates.
- Smokes: `deadreckon import codex --preview` creates no run; `deadreckon import codex --session <fixture>` creates one imported run; `deadreckon import cursor` still round-trips.
- New fixtures cover Codex, Claude Code, Gemini, OpenCode file mode, Copilot, and Pi using descriptor-backed discovery without touching real home directories.
- Ambiguous and stale-session cases refuse with concrete `try:` lines.
- No edits outside `/Users/gdc/deadreckon/`. No provider transcript rewrites. No `PipelineState` schema changes.

**Stop when** verification passes, AS-BUILT and CHANGELOG describe "Descriptor import hardening (alpha)", deferred SQLite/richer replay/import-analytics scope is in `docs/V1-CANDIDATES.md`, and the work is committed locally.
