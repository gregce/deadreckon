GOAL: Reclaim DeadReckon's test suite for CI, prove the Windows artifact actually runs, and finish the repository re-home — closing the three loose ends Release Trust left open. `v0.1.0-rc.1` now publishes signed, notarized, attested artifacts, but `release-verify` is narrowed to fmt+build because the workspace test suite only runs on the author's macOS (hardcoded `/Users/gdc` paths in tests and shipped source, BSD `script` PTY across five test files); the Windows target builds but is unsigned and never run; and the `gdc`→`gregce` re-home stopped at the gate and metadata. Land a follow-up slice named Release Trust Closure.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-06-04-1951-deadreckon-release-trust-closure-rider.md` - phases, portability rules, depth tests.
- `/Users/gdc/deadreckon/docs/goals/2026-06-01-1523-deadreckon-release-trust-goal.md` and `-rider.md` - parent slice; invariants hold.
- `/Users/gdc/deadreckon/.github/workflows/release.yml` - the only workflow; `release-verify` holds the narrowed gate.
- `/Users/gdc/deadreckon/release/trust/release-trust.mjs` - lane classifier, `requires_windows_signing`, preflight.
- `/Users/gdc/deadreckon/dist-workspace.toml` - target matrix, `tap`.
- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`, `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`, `/Users/gdc/deadreckon/docs/RELEASE.md`.

**Posture.** Release trust hardening; no CLI redesign. No runtime-state schema changes. No `git push`, tag push, or real package/signing publish by the agent. Edits inside `/Users/gdc/deadreckon`. Forks/PRs stay usable without secrets; official tags fail closed when trust material is missing. Canonical repo is `gregce/deadreckon` (where `rc.1` published); the re-home completes to it.

**Three workstreams.**

- **Portable suite.** No test or shipped-source code depends on `/Users/gdc`, an absolute home, or BSD `script`. Path/home resolution derives from env or the OS home; PTY tests use a cross-platform helper or are cfg-gated honestly. A guard test forbids new hardcoded `/Users/` in the tree.
- **Real test gate.** Add `.github/workflows/ci.yml` running `cargo fmt --check`, `cargo clippy`, and `cargo test --workspace` on Linux; restore `release-verify` to run the suite once it is green. No release may ship on a suite that never ran.
- **Windows + re-home.** A `windows-latest` job builds and runs `deadreckon --version` and `deadreckon doctor`, failing closed if the binary does not execute; unix-only feature gaps are documented. Stable Windows stays blocked unless Authenticode secrets are present (`requires_windows_signing` fail-closed), with the wiring in RELEASE.md. Finish `gdc`→`gregce` in `providers.rs`, `update_cache.rs`, and the `dist-workspace.toml` `tap`.

**Phases.** Eleven in the rider. Each: write the named depth test(s) first and watch them fail; make the narrow change; `cargo fmt --check`, `cargo clippy`, and focused `cargo test` green; conventional-commit locally; one-line CHANGELOG. P11 updates AS-BUILT, RELEASE, V1-CANDIDATES, CHANGELOG.

**Verification.**

- Every rider depth test present and passing; `cargo test --workspace` green on Linux in the new CI workflow.
- `grep -rn "/Users/" crates` returns nothing outside fixtures the guard test explicitly allows.
- `deadreckon --version` and `deadreckon doctor` exit 0 on `windows-latest` in CI.
- `release-trust.mjs` lane/preflight tests still assert stable Windows fail-closed; `dist plan` / the release-plan test stays green; `cargo fmt --check` and `git diff --check` clean.

**Stop when** the workspace suite runs green in CI on Linux and gates releases, the Windows binary is proven to execute (or its gaps are documented and stable Windows stays blocked), the re-home is consistent, verification passes, AS-BUILT/RELEASE/V1-CANDIDATES/CHANGELOG are updated, and the work is committed locally.
