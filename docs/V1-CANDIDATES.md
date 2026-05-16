# V1 Candidates

- Explicit sub-agent forking command: `deadreckon fork <run-id> --prompt "..."`, from AS-BUILT §10 and REPORT.md coordination needs.
- Provider HTTP retry taxonomy: `ProviderError::Http` currently carries provider/detail text but no HTTP status field, so the hygiene taxonomy treats it as fatal. Add a status/code field before retrying 408, 429, or 5xx provider failures.
- OpenCode SQLite ingest: current provider CLI ingest reads OpenCode file-mode `storage/session`, `storage/message`, and `storage/part` JSON only. Add SQLite-backed discovery/parsing after choosing a dependency-light strategy and fixture shape.
- Provider transcript undo/bulk-agent registration: provider-owned logs stay read-only in alpha. Any undo/replay, mass registration, or transcript mutation workflow needs a separate design that preserves provenance and provider ownership boundaries.
- CLI stdout/run-output ingest and stdin prompt transport: Copilot and Pi now use saved provider sessions for the attach TUI. If a future provider needs `--no-session`, run-local JSONL stdout parsing, per-run `--session-dir`, or stdin prompt delivery for very large prompts, extend the descriptor schema with focused fixtures instead of adding one-off adapters.
- Rich semantic merge UI: alpha merge repair is file-backed and CLI-first. A V1 UI could present side-by-side conflict versions, planner rationale, dependency graph context, and approval controls without requiring users to inspect `merge-proofs/` JSON directly.
- AST/language-aware merge engines: alpha repair uses DAG precedence plus provider-planned file decisions. V1 can add parser-backed merges for common languages when the dependency weight and failure modes are clear.
- Broadcast-backed plan event bus: alpha plan events are durable JSONL. A V1 multiplexer could stream plan, child, and repair events to attach sessions without polling plan/run files.
- Mass rename of stored status variants, especially `RunStatus::Executing` to `RunStatus::Running`. The coherence pass changed display text only.
- Themable palettes via config. Alpha keeps one hard-coded palette in `ui.rs`.
- Localization hooks for status words, nouns, prompts, and command hints.
- Migration from hand-built status cards to a small template engine once the alpha CLI layout settles.
- Full command-family renames beyond hidden alpha aliases, including removing `--force`, `--all`, `--branch`, and `--budget-cap` after the alias window closes.
