GOAL: Audit deadreckon at `/Users/gdc/deadreckon/` against the 25 original unmet needs and the as-built reality, then close the highest-leverage hardening + usability gaps the audit surfaces. AS-BUILT §22 names ten scaffolding-thin items; recent UX commits hint at follow-up polish. Headline word: **Hardening**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` — substrate; especially §22.
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-deadreckon-audit-harden-rider.md` — audit shape, schemas, depth tests, eleven-phase plan.
- `/Users/gdc/stoa/docs/research/2026-05-10-unmet-needs/REPORT.md` — the original 25 needs.
- `/Users/gdc/deadreckon/CHANGELOG.md`, `docs/GAP-ANALYSIS.md`, `docs/V1-CANDIDATES.md`.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` — invariants hold.

**Posture.** Stays `alpha`. No `PipelineState` schema changes (state lives in files). No `git push`. Edits inside `/Users/gdc/deadreckon/`. Major architectural decisions → `docs/V1-CANDIDATES.md`.

**One audit + nine closures + a doc pass.**

- **`docs/AUDIT-2026-05-11.md`** — one row per unmet need: status (Resolved / Partial / Unmet / V1), evidence (file:line, command, or doc path), recommendation. Drives the closure phases; revisited at P11 to show before/after.

**Nine concrete closures.**

- **TUI streaming.** `attach` subscribes to `RunEventBus`; falls back to `events.jsonl` replay for cross-process attach.
- **Cross-process kill.** `kill` writes a cancel marker the run loop watches between turns and inside HTTP requests; cancels in-flight `reqwest` in another process.
- **Mid-tool-call resume.** Truncated `traces.jsonl` replays the partial tool boundary instead of advancing past it.
- **Sandbox per-tool policy.** A `sandbox.toml` gates `bash` / `write_file` / network per-tool; refusal includes `try:`.
- **Acceptance YAML spec.** `dr-gate` reads `acceptance.yaml` (`tests`, `file-exists`, `content-match`, `build-success`, `shell`); per-check results in `proofs/turn-acceptance.json`.
- **Doctor exhaustive.** Opt-in provider-ping (`DEADRECKON_DOCTOR_PING=1`), OS/kernel sanity, write-perm checks, `claude` / `codex` version probes; every line ends `try:`.
- **Library query.** `deadreckon library list|search|show` over `~/.deadreckon/library/` with goal/scope/date filters and grep across promoted run docs.
- **Import round-trip parity.** Each claude/codex/cursor turn lands a normalized trace + provenance the `show` verb renders identically; covered by golden-file tests.
- **Help discoverability + status polish.** `--help` groups verbs by lifecycle stage; post-action hints carry the next verb; `status` summarizes promoted-library count and disk usage.

**Friendliness as a verifiable contract.**

- Auto-detect, don't ask (audit generation, library defaults to current scope).
- Preflight + preview before any state change (policy reload, library purge).
- Refuse with `try: <command>` on every error footer.
- Rollback is one command (`undo` / `discard` / `cleanup`).
- Lifecycle hints after every action.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check` green → conventional-commit → CHANGELOG. P11 adds a "Hardening v2" section to AS-BUILT and updates §22.

**Verification.**

- Commands green every commit; every rider depth test present and passing.
- Audit smoke: `docs/AUDIT-2026-05-11.md` lists all 25 needs with a status + evidence path each.
- TUI smoke: a kill-mid-turn surfaces `RunCompleted{killed}` in `attach` within 200 ms.
- Sandbox smoke: a per-tool policy refusing `bash` reading `~/.ssh/id_rsa` returns `try:` and a `provenance.jsonl` refusal entry.
- No edits outside `/Users/gdc/deadreckon/`. No `git push`. No `PipelineState` schema changes.

**Stop when** verification passes, AS-BUILT updated, CHANGELOG has a "Hardening v2 (alpha)" section, committed locally.
