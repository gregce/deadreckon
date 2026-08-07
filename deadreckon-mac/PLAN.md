# deadreckon-mac — build program

Executes `docs/design/MAC-APP-OPERATOR-CONSOLE.md` (§6.2 composite: Bridge shell, Quarterdeck home, Chartroom depth, converged Binnacle). Branch: `deadreckon-mac`. Phased commits, no pushes.

## Operator decisions (2026-08-06)

1. **App name:** deadreckon (the Mac app carries the product name).
2. **Repo home:** this repo, `deadreckon-mac/`.
3. **IA:** the §6.2 recommendation as written.
4. **Write parity:** the app (including the menubar popover) supports the same write verbs the CLI does — popover is not triage-only.
5. **Steering:** G6 plus widening — steer must work for any supported provider (between-turn steer-inbox consumption in the turn loop; codex-server keeps its mid-turn path).
6. **Notifications:** real user notifications, including the binary-side work (typed operator-attention events).
7. **Distribution:** straight to notarized releases, signed the same way deadreckon is (Developer ID Application: 4GRQMF5T5U; existing release-trust pipeline).
8. **Queue ranking:** durable facts only. No forward-looking estimates.

**Validation exemplar:** `/Users/gdc/getspecstory/specstory-mac` (note: `/users/gdc/getspecstoryai/mac-app` does not exist on this machine).

## Phases

| Phase | Scope | Status |
|---|---|---|
| R-M0 | G6 steerable-as-data · G8 live gate progress · G7 receipt audit · G10 diff patches · G3 fleet rollup · TAILING.md contract | **done** (trust review fixed 3 HIGH: marker-only attempt-scoped gate counts; fail-fast strict receipt path) |
| R-M1 | G1 JSON envelopes (9 verbs + global error envelope) · G2 start parity · G9 extend --note · steer widening (all providers) · notify events | in progress |
| R-M2 | G4 finish --dry-run --json · finish --json | pending |
| R-M3 | G5 follow verb (merged NDJSON, replay offsets) | pending |
| APP-1 | Scaffold: project.yml, DeadreckonKit, CONTRACTS.md, vendor-cli.sh + manifest | **done** (BUILD SUCCEEDED; 35 Kit tests; exemplar-fidelity reviewed) |
| APP-2 | Menubar shell + Gate Queue home + Harbor + ⌘K | pending |
| APP-3 | Chartroom workbench (narrative/spine/turns, evidence rail, drawer) | pending |
| APP-4 | Write parity: Lay Course, steer, kill, Binnacle promote, send-back, popover writes | pending |
| APP-5 | User notifications (stable IDs, catch-up), popover, polish | pending |
| VALIDATE | Build+tests green; UX/menu-flow benchmark vs specstory-mac; iterate until GREAT | pending |
| RELEASE | codesign (Developer ID 4GRQMF5T5U) + notarytool, riding the deadreckon release-trust pipeline | pending |

One commit per phase (more if a phase lands in coherent slices). Trust rules from the design doc hold throughout: the app never invokes dr-gate, never signs, no override affordances, gate-keys never read; dry-run promote is report-only.
