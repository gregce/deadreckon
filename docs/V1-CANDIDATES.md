# V1 Candidates

- Explicit sub-agent forking command: `deadreckon fork <run-id> --prompt "..."`, from AS-BUILT §10 and REPORT.md coordination needs.
- Provider HTTP retry taxonomy: `ProviderError::Http` currently carries provider/detail text but no HTTP status field, so the hygiene taxonomy treats it as fatal. Add a status/code field before retrying 408, 429, or 5xx provider failures.
- OpenCode SQLite ingest: current provider CLI ingest reads OpenCode file-mode `storage/session`, `storage/message`, and `storage/part` JSON only. Add SQLite-backed discovery/parsing after choosing a dependency-light strategy and fixture shape.
- Provider transcript undo/bulk-agent registration: provider-owned logs stay read-only in alpha. Any undo/replay, mass registration, or transcript mutation workflow needs a separate design that preserves provenance and provider ownership boundaries.
- CLI stdout/run-output ingest and stdin prompt transport: Copilot and Pi now use saved provider sessions for the attach TUI. If a future provider needs `--no-session`, run-local JSONL stdout parsing, per-run `--session-dir`, or stdin prompt delivery for very large prompts, extend the descriptor schema with focused fixtures instead of adding one-off adapters.
- Rich semantic merge UI: alpha merge repair is file-backed and CLI-first. A V1 UI could present side-by-side conflict versions, planner rationale, dependency graph context, and approval controls without requiring users to inspect `merge-proofs/` JSON directly.
- AST/language-aware merge engines: alpha repair uses DAG precedence plus provider-planned file decisions. V1 can add parser-backed merges for common languages when the dependency weight and failure modes are clear.
- Plan event bus hardening: plan attach now consumes a `PlanEventBus` feed over durable JSONL replay/tail plus a broadcast-capable runtime API, and it multiplexes child and repair run events. Future work can wire every same-process plan writer through a long-lived broadcaster if an embedded attach mode needs zero-file-hop delivery.
- Mass rename of stored status variants, especially `RunStatus::Executing` to `RunStatus::Running`. The coherence pass changed display text only.
- Themable palettes via config. Alpha keeps one hard-coded palette in `ui.rs`.
- Full output-layout facade: universal key/value rows, `try_line`/`next_action` helpers, table header helpers, stream-policy enforcement, and a generic lifecycle summary renderer for run/plan/chain/finish/apply/export/extend/resume/kill/cleanup.
- Orchestration UI polish beyond the live slice: richer interactive mode/provider/done-criteria setup, a fuller output-layout facade, and deeper golden snapshots once the CLI layout settles. The live slice already added shared plan/fork/merge/orchestrate summaries, role tables, dependency/parallelism summaries, standard plan attach footers, and structured merge-repair panels.
- Provider and done-criteria setup unification: one reusable provider selection/prompt flow across `init`, `config provider`, run/orchestrate flags, doc polish, and one docs/help source for `def-done` plus the hidden `acceptance` compatibility surface.
- Command-matrix golden snapshots for help, summaries, prompts, table output, and JSON/plain/no-hints behavior once the CLI layout settles enough that snapshots catch regressions without making normal copy edits brittle.
- Localization hooks for status words, nouns, prompts, and command hints.
- Migration from hand-built status cards to a small template engine once the alpha CLI layout settles.
- Full command-family renames beyond hidden alpha aliases, including removing `--force`, `--all`, `--branch`, and `--budget-cap` after the alias window closes.
