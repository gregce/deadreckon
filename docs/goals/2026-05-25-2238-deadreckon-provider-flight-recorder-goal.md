GOAL: Capture provider-native subturns and safe checkpoints for CLI-backed runs so `cli:*` providers stop looking like one opaque DeadReckon turn. Today DeadReckon snapshots before/after a CLI subprocess, records one `tool.cli_subagent`, and shows live provider logs through descriptor ingest. Land a provider flight recorder that persists provider-native events, correlates them to working-tree checkpoints, and adds preview-first rewind without pretending provider events are normal DeadReckon turns. Headline word: **Recoverable**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - telemetry, import, TUI, CLI provider model.
- `/Users/gdc/deadreckon/docs/design/PROVIDER-CLI-INGEST.md` - descriptor ingest rules.
- `/Users/gdc/deadreckon/docs/goals/2026-05-25-2238-deadreckon-provider-flight-recorder-rider.md` - implementation contract.
- Provider ingest riders from 2026-05-13 - ingest invariants still hold.
- Current code: runtime turn loop, core artifacts, CLI `main.rs`, and provider registry/descriptors.
- `/Users/gdc/stoa/docs/research/2026-05-10-unmet-needs/REPORT.md` - undo, provenance, observability, spend/context pain.

**Posture.** Stays `alpha`. No `PipelineState` schema changes. Do not rewrite or mutate provider-owned logs. Keep DeadReckon turns as the outer mutation boundary; provider-native events are subturns inside a turn. No `git push`. Edits stay inside `/Users/gdc/deadreckon/`. V1-scale decisions go to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**Core deliverables.**

- Add per-run `flight-events.jsonl` and `flight-manifest.json` for CLI-backed runs. They normalize descriptor-ingested provider rows into ordered provider-native events with source path/line/hash, provider, schema, DeadReckon turn, kind, tool category, files, usage, and optional checkpoint id.
- Add a live recorder sidecar around CLI provider execution. It tails descriptor-discovered provider logs, watches the working tree, and emits checkpoints after mutation-like activity or quiet file changes.
- Add checkpoint storage under `<run_root>/checkpoints/` using delta checkpoints plus occasional full anchors. Checkpoints must preserve created, modified, and deleted files and be reconstructable without provider logs.
- Add `deadreckon show <run-id> --flight` and `deadreckon show <run-id> --file <path>` views that explain which provider subturns and checkpoints touched a file.
- Add `deadreckon rewind <run-id> --to-turn <n>|--to-provider-event <seq>|--to-checkpoint <id> --preview|--apply`. Rewind hash-guards current files and refuses unrelated user edits.
- Integrate flight events into attach/TUI while keeping provider activity lines as the live fallback.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implement -> focused verification -> conventional local commit -> CHANGELOG. P11 updates AS-BUILT, V1-CANDIDATES, and docs for what is still not exact subturn control.

**Verification.**

- Focused matrix only: new flight/checkpoint unit tests, provider-ingest fixtures, rewind preview/apply tests, show/TUI rendering tests, fmt, clippy for touched crates, and targeted cargo tests for touched packages.
- Smokes: fake CLI provider writes provider JSONL plus file edits; `show --flight` displays provider-native events; `rewind --to-provider-event ... --preview` shows exact file changes; `--apply` restores only guarded files.
- Do not run `make verify`, release builds, stress tests, smoke suites, or full-workspace tests by default.

**Stop when** focused verification passes, the recorder/checkpoint/rewind flow is documented in AS-BUILT and CHANGELOG, known non-goals are in V1-CANDIDATES, and the work is committed locally.
