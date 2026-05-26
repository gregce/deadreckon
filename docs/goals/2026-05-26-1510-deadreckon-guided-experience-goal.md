GOAL: Make DeadReckon feel obvious, welcoming, and hard to misuse by landing a guided front door for running or orchestrating goals plus a sharper product identity. Today the substrate is strong, but the user must still learn too much command topology before the first serious run. This goal makes the answer immediate: DeadReckon is for people who already trust agent CLIs and need unattended, sandboxed, auditable work with a real definition of done. Headline word: **Guided**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - substrate and CLI surface.
- `/Users/gdc/deadreckon/docs/goals/2026-05-26-1510-deadreckon-guided-experience-rider.md` - launcher behavior, copy contract, depth tests.
- `/Users/gdc/deadreckon/docs/design/USER-FACING-MATRIX.md` - UX deferrals.
- `/Users/gdc/deadreckon/README.md` and `/Users/gdc/deadreckon/HOWTO.md` - current public framing and first-run path.
- Prior setup, orchestration, coherence, event bus, and self-improvement riders - invariants hold.

**Posture.** Stays `alpha`. No `PipelineState` schema changes. Prefer shared renderers and ephemeral launch decisions over durable state. Do not weaken safety invariants. No `git push`. Edits stay inside `/Users/gdc/deadreckon/`. Major product or schema decisions go to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**Audience contract.**

- Primary users: builders, maintainers, founders, and engineering leads who already use agent CLIs for longer, riskier work.
- DeadReckon is the harness around those harnesses: it owns isolation, done criteria, lifecycle, logs, evidence, recovery, and promotion.
- Not the promise: chat app, cloud IDE, provider replacement, magic autonomous employee, or unsupervised deploy tool.
- This framing must appear in top help, README, HOWTO, and setup recovery copy.

**Guided front door.**

- Add `deadreckon start "<goal>"` as the recommended new-user path. It resolves provider setup, done criteria, source mode, and run-vs-orchestrate choice, then starts or previews the selected flow.
- Existing `run` and `orchestrate` remain canonical power-user commands.
- `start` defaults to no provider flags: use configured defaults, detect installed providers only when defaults are missing, and print exact `try:` lines when setup is incomplete.
- `start --mode auto|run|review|full-plan`, `--preview`, `--yes`, `--plain`, `--quiet`, and `--json` follow existing output contracts.
- Auto mode names the selected path, reason, and override flag.

**Friendliness as a verifiable contract.**

- One command from an ordinary repo must either start a run/plan or end with concrete `try:` lines.
- Missing provider, no done criteria, non-git directory, dirty repo, sandbox limits, or high budget must lead to guided recovery.
- Every start/run/orchestrate preview says who works, where it runs, how done is checked, how to watch/stop it, and what finishes it.
- JSON/plain/quiet behavior stays coherent and covered.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implement -> focused verification green -> conventional local commit -> CHANGELOG. P11 updates AS-BUILT, README, HOWTO, and V1-CANDIDATES.

**Verification.**

- Focused tests only by default: command parsing, help/catalog snapshots, preflight builders, start auto-resolution, shared output, JSON/plain/quiet, and docs copy assertions.
- Smokes: `deadreckon start --preview "goal"`, missing-provider recovery, non-git recovery, and `start --mode review --preview` produce actionable output without provider-specific flags.
- Do not run `make verify`, release builds, stress tests, broad smoke suites, or full-workspace tests by default while executing this goal unless the human explicitly asks.

**Stop when** focused verification passes, docs make the audience and first path obvious, the guided launcher works in preview and recovery cases, AS-BUILT and CHANGELOG record alpha limits, and the work is committed locally.
