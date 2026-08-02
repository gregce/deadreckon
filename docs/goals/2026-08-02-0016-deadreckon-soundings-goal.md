GOAL: Land Soundings: make `start` resolve and freeze the real source before drafting done criteria, keep preview and dispatch on one decision, support `--from` for review/full-plan Graph Jobs, and bound authoring so a tool-wandering provider cannot hold admission indefinitely. The reproduced path spent fourteen minutes drafting against an empty destination, invented `FlappyBird` while the source product was `Cloudwing`, wrote the accepted contract, then rejected the previewed `--from` at final dispatch. Soundings makes preflight source-true, fast-failing and time-bounded.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-08-02-0016-deadreckon-soundings-rider.md`.
- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` §§46, 48 and 58.
- `/Users/gdc/deadreckon/docs/goals/2026-07-03-1304-deadreckon-contract-rider.md` and `/Users/gdc/deadreckon/docs/goals/2026-07-28-2321-deadreckon-watchkeeper-rider.md`.
- `/Users/gdc/deadreckon/crates/deadreckon/src/commands/start.rs`, `acceptance.rs`, `job.rs`, `graph_job.rs`; `/Users/gdc/deadreckon/crates/deadreckon-providers/src/types.rs`, `cli_common.rs`, `cli_codex.rs`.
- `/Users/gdc/deadreckon/docs/MAP-OF-DEADRECKON.md`.

**Posture.** Stable track. No `Job`, `PipelineState`, launch-plan or acceptance-file schema changes. Reuse `DurableSource`, four check kinds, Watchkeeper authority and `doc_provider`. New state is in-memory or controller-owned below the Job root. No push, release, live paid dogfood or edits outside `/Users/gdc/deadreckon`.

**Settled behavior.**

- Resolve mode/source once before provider calls, writes or confirmation. Preview, acceptance, authority and dispatch consume it.
- `start --mode review|full-plan --from <dir>` snapshots tracked and untracked deliverables into controller-owned approved source; Graph children isolate from it and never modify the original.
- Acceptance has separate write and inspection roots: artifacts remain in the launch project, while the bounded project dossier comes from the resolved source. Generated commands stay portable through `{working_dir}`.
- Structured-text-only draft → critic → optional redraft share one 120-second wall budget. Timeout reaps the child tree and never approves a weak partial contract.
- Redraft receives prior YAML, Markdown, helpers and full verdict. `reject` normalizes to `redraft`; one critic/redraft remains the ceiling.
- Valid generated artifacts are reusable; deterministic incompatibilities fail before authoring.

**Phases.** Eleven (P1–P11) in the rider. Each: named depth tests watched red → implement → focused green tests (`make verify` at milestones) → conventional local commit → CHANGELOG. P11 adds AS-BUILT §59 and updates the map.

**Verification.**

- The exact empty-destination + dirty/untracked Swift `--from` fixture previews and launches one Graph Job; its frozen source contains `Package.swift`, `Cloudwing`, sources and tests, and the external source remains byte-identical.
- Unsupported inputs reach no provider, confirmation or write; preview and dispatch cannot disagree.
- The acceptance prompt sees `Cloudwing`, never needs web/schema discovery, uses output schema, and a redraft sees its predecessor and exact critic findings.
- A never-returning draft/redraft stops within budget, reaps descendants, writes nothing partial and prints one `try:` line.
- Existing Single, clean-worktree Graph, preview/JSON/plain, Contract lint/critic and Watchkeeper authority/receipt suites remain green.

**Stop when** all rider depth tests and `make verify` pass, the reproduced command returns a Job ID without `--from` contradiction, bounded latency evidence exists, AS-BUILT §59/MAP/CHANGELOG are honest, an operator checklist is written, and changes are committed locally.
