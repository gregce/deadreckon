GOAL: Add GitHub Copilot CLI and Pi as first-class Deadreckon CLI providers, registered, detectable, routable, and visible in the attach TUI like the existing CLI providers. The provider-registry and provider-ingest work already made descriptors, generic CLI launch, and schema-keyed TUI parsing possible; this goal lands the next two real CLIs using `/Users/gdc/gnhf` and `/Users/gdc/agentsview` as research. Copilot should use its JSON output and `~/.copilot/session-state` logs. Pi should use its installed CLI flags and `~/.pi/agent/sessions` JSONL sessions. Headline word: **Broader**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - provider/TUI substrate.
- `/Users/gdc/deadreckon/docs/goals/2026-05-13-1705-deadreckon-copilot-pi-providers-rider.md` - exact descriptors, parsers, and tests.
- `/Users/gdc/deadreckon/docs/goals/2026-05-13-1345-deadreckon-provider-cli-ingest-rider.md` - registry/ingest invariants and focused verification ladder.
- `/Users/gdc/gnhf/src/core/agents/{copilot.ts,pi.ts}` - launch/output behavior.
- `/Users/gdc/agentsview/internal/parser/{types.go,discovery.go,copilot.go,pi.go}` - discovery/parser prior art; port ideas, no runtime dependency.
- Current seams: `crates/deadreckon-providers/{descriptors,src/registry,src/cli_generic.rs,tests}` and `crates/deadreckon/src/main.rs`.

**Posture.** Stays `alpha`. No `PipelineState` schema changes and no rewrites of provider-owned transcripts. Descriptor schema may grow only if needed for Pi stdin/session-dir or run-output ingest, and that growth must be tested against existing descriptors. No `git push`. Edits inside `/Users/gdc/deadreckon/`. Bigger architecture choices go to `docs/V1-CANDIDATES.md`.

**Deliverables.**

- Built-in `cli:copilot` descriptor: `copilot` binary, subscription auth, sandbox read/write `~/.copilot`, JSON non-interactive launch, model override, install hints, detection, and `[ingest] schema = "copilot-cli"`.
- Built-in `cli:pi` descriptor: `pi` binary, subscription/API-key auth posture, sandbox read/write `~/.pi/agent`, JSON non-interactive launch, model override, install hints, detection, and `[ingest] schema = "pi"`.
- Registry, `detect`, `providers list --all`, init/provider selection, and generic-router tests prove both providers register and route without new `ProviderKind` variants unless Pi genuinely requires a concrete adapter.
- TUI ingest parses Copilot and Pi activity into the same common rows as existing providers: `agent`, `thinking`, `tool`, `result`, and `tokens`.
- Copilot discovery covers `~/.copilot/session-state/*.jsonl` and `~/.copilot/session-state/*/events.jsonl`; Pi discovery covers `~/.pi/agent/sessions/<encoded-cwd>/*.jsonl`.
- Binary probes are harmless: `copilot --help` and `pi --help` or `--version` only. No live model calls in tests.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implement -> run only the rider's focused verification for touched crates -> conventional local commit -> CHANGELOG. P11 updates AS-BUILT provider/TUI sections and built-vs-thin accounting.

**Verification.**

- Do **not** run `make verify`, release builds, smoke, stress, or full-workspace tests by default. Use the rider's targeted commands per phase.
- Registry smoke: `cli:copilot` and `cli:pi` appear in built-ins with descriptors round-tripping.
- CLI smoke: fake `copilot` and `pi` binaries prove generic routing, args, model flags, output capture, wall-time subscription spend, and sandbox writes.
- TUI smoke: Copilot and Pi fixtures produce normalized provider activity and context/token telemetry without touching real home logs.
- CLI UX smoke: `deadreckon detect` and `deadreckon providers list --all` include both providers and useful `try:` install lines.

**Stop when** targeted verification passes, AS-BUILT and CHANGELOG describe "Copilot and Pi providers (alpha)", deferred stdin/run-output/session-dir questions are in `docs/V1-CANDIDATES.md`, and the work is committed locally.
