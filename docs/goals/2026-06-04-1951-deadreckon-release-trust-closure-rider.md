# DeadReckon — Release Trust Closure Rider (portable suite, real test gate, Windows + re-home)

This rider holds the prescriptive constraints for the goal at `/Users/gdc/deadreckon/docs/goals/2026-06-04-1951-deadreckon-release-trust-closure-goal.md`. It supersedes nothing in the parent Release Trust rider (`2026-06-01-1523-deadreckon-release-trust-rider.md`) or the distribution rider (`2026-05-11-2343-deadreckon-distribute-rider.md`); their invariants still apply. This rider closes the three loose ends that surfaced when the pipeline first published a real artifact (`v0.1.0-rc.1`): the test suite never ran in CI, the Windows artifact is unproven, and the `gdc`→`gregce` re-home is partial.

**All paths absolute.** Source root is `/Users/gdc/deadreckon`. Runtime state under `/Users/gdc/.deadreckon` is not part of this goal. CI artifacts live under `target/distrib`; local tests use tempdirs only.

## Posture (decided — do not redesign)

- **Maturity stays production-release track.** Hardening and CI reclamation, not a new install channel or CLI UX redesign.
- **Canonical repository is `gregce/deadreckon`.** That is where `v0.1.0-rc.1` published; `OFFICIAL_REPO` in `release-trust.mjs` already says `gregce/deadreckon`. The re-home completes to it. Do not reintroduce `gdc/deadreckon` except as historical references in CHANGELOG/runbook prose.
- **No DeadReckon durable runtime schema changes.** Do not change `PipelineState`, plans, campaigns, chains, library artifacts, run markers, or install receipts. Path/home *resolution* may change; the persisted shapes may not.
- **Official releases stay fail-closed.** Stable Windows must not publish unsigned; missing Authenticode secrets block stable Windows artifacts, never silently ship them.
- **Forks and PRs remain usable.** The new test CI runs without any release secret. Windows smoke needs no signing secret.
- **No agent-driven publishing or signing.** The agent must not run `git tag`, `git push`, `gh release upload`, `npm publish`, Homebrew tap pushes, or real Authenticode signing outside tests/dry-runs.
- **No secret material in git.** Docs may name secret keys and commands, never values, certificates, passwords, or team identifiers.
- **Edits stay inside `/Users/gdc/deadreckon`.**

## Current baseline and gaps (verified at HEAD)

- `/Users/gdc/deadreckon/.github/workflows/release.yml`
  - `release-verify` step "Full release verification" (lines ~127–136) runs `cargo fmt --check` then `cargo build --release --workspace --locked`. The `cargo test --workspace` gate was removed because the suite fails on Linux.
  - There is **no other workflow**; `release.yml` is the only CI. The test suite has therefore never run in CI.
- Machine-coupled **shipped source** (compiled into the binary):
  - `crates/deadreckon-core/src/paths.rs:6` `DEFAULT_DEADRECKON_HOME = "/Users/gdc/.deadreckon"`, `:7` `SOURCE_ROOT = "/Users/gdc/deadreckon"`.
  - `crates/deadreckon-providers/src/config.rs:10` `DEFAULT_CONFIG_PATH = "/Users/gdc/.deadreckon/config.toml"`.
  - Further `/Users/gdc` literals in `crates/deadreckon-core/src/learning.rs`, `crates/deadreckon/src/commands/learning.rs`, `crates/deadreckon/src/commands/chain/mod.rs` (hook paths), `crates/deadreckon-runtime/src/turn_loop.rs` — classify each: a real default needs portable resolution; pure test-data strings only need the guard test to tolerate them.
- BSD/macOS `script` PTY emulation in tests, broken on Linux `util-linux` `script`:
  - `crates/deadreckon/tests/chain.rs` (5), `codebase.rs` (3), `agentic_loop.rs` (14), `orchestrate.rs` (18), `lifecycle.rs` (25 references).
  - `crates/deadreckon/tests/audit_harden.rs` external `/Users/gdc/stoa/...REPORT.md` cross-ref is already quarantined (skips when the file is absent) — keep that pattern.
- Windows: `release-trust.mjs:129` `requires_windows_signing = official_repo && lane === "stable"`; preflight (`:185`) requires `WINDOWS_CERT_PFX`/`WINDOWS_CERT_PWD` only for stable. The `x86_64-pc-windows-msvc` build packages `deadreckon.exe` + `dr-gate.exe` but is never executed anywhere.
- Re-home leftovers: `crates/deadreckon/src/commands/providers.rs:545,573,590,609` (`gdc/deadreckon` release/update URLs, including `api.github.com/repos/gdc/deadreckon/releases/latest`); `crates/deadreckon-core/tests/update_cache.rs:15,30` test data; `dist-workspace.toml:18` `tap = "gdc/homebrew-tap"` (formula receipt emits `brew:gdc/tap/deadreckon`).

## Portability rules (the spec for "machine-coupled")

- **Home resolution.** A shipped default that points at a home directory must resolve at runtime from, in order: an explicit env override (e.g. `DEADRECKON_HOME`), then the OS home (`std::env::var_os("HOME")` / `dirs`-style), then a documented fallback. No absolute `/Users/<name>` literal may remain in shipped `src/`.
- **Source root.** `SOURCE_ROOT` is a development convenience; if it must stay, derive it (e.g. `CARGO_MANIFEST_DIR`-relative or env) rather than hardcoding one machine. If nothing in shipped code paths reads it at runtime, move it behind `#[cfg(test)]` or delete it.
- **PTY tests.** Replace direct `Command::new("script")` BSD-syntax invocations with one shared helper that either (a) uses a portable pty crate, or (b) dispatches per-OS (`#[cfg(target_os = ...)]`) to the correct `script` syntax (macOS `script -q /dev/null cmd ...` vs Linux `script -qec "cmd" /dev/null`). The helper lives in one place; the five test files call it. A test that genuinely cannot run on Linux is `#[cfg_attr(not(target_os = "macos"), ignore)]` with a comment — not silently skipped.
- **Guard.** A single guard test greps the `crates/` tree for new hardcoded `/Users/` literals and fails on any not on an explicit allowlist (the allowlist names the few remaining test-data strings, each with a justification). New machine coupling cannot regress in.

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them fail; make the narrow change; `cargo fmt --check`, `cargo clippy --workspace --all-targets`, and the focused `cargo test` green; conventional-commit locally; one-line CHANGELOG entry naming the SHA.

### P1 — Finish the re-home to gregce/deadreckon

- Replace `gdc/deadreckon` with `gregce/deadreckon` in `crates/deadreckon/src/commands/providers.rs` (release/update URLs incl. the `api.github.com/repos/...` defaults) and `crates/deadreckon-core/tests/update_cache.rs` test data.
- Decide the Homebrew tap: set `dist-workspace.toml` `tap` to the canonical `gregce/homebrew-tap` (or document why it stays `gdc`); ensure the formula receipt `install_source` matches.
- Depth tests (in `crates/deadreckon/tests/update_cli.rs` and a `providers` unit test):
  - `update_default_release_url_targets_canonical_repo`
  - `providers_latest_release_api_url_targets_canonical_repo`

### P2 — Portable home/path resolution in shipped source

- Replace the `/Users/gdc` literals in `crates/deadreckon-core/src/paths.rs` and `crates/deadreckon-providers/src/config.rs` with env-or-home resolution per the portability rules. Preserve existing public behavior (same effective path on the author's machine) while removing the hardcoded literal.
- Depth tests (in `crates/deadreckon-core/src/paths.rs` tests):
  - `default_home_resolves_from_env_then_os_home_not_hardcoded`
  - `default_config_path_has_no_absolute_user_literal`

### P3 — Cross-platform PTY helper

- Add one shared test helper (e.g. `crates/deadreckon/tests/support/pty.rs` or a `dev-dependency` pty crate wrapper) that runs a command under a pseudo-TTY on both macOS and Linux. No test file calls `script` directly after this slice lands.
- Depth tests (in the helper's own test module):
  - `pty_helper_runs_command_under_tty_on_host`
  - `pty_helper_reports_tty_true_to_child`

### P4 — Port chain.rs PTY tests

- Convert `crates/deadreckon/tests/chain.rs` to the P3 helper; fix any `/Users` path coupling there.
- Depth tests: `chain_pty_tests_run_on_linux` plus the existing `chain_*` PTY tests now passing under the helper.

### P5 — Port codebase.rs and agentic_loop.rs

- Convert `codebase.rs` and `agentic_loop.rs` (including the import golden tests' run-id redaction, which is line-wrap-fragile) to the helper and to path-agnostic redaction.
- Depth tests: `agentic_loop_import_golden_redaction_is_wrap_independent`; `codebase_pty_tests_run_on_linux`.

### P6 — Port orchestrate.rs and lifecycle.rs

- Convert the two largest PTY suites (`orchestrate.rs`, `lifecycle.rs`) to the helper.
- Depth tests: `orchestrate_pty_tests_run_on_linux`; `lifecycle_pty_tests_run_on_linux`.

### P7 — Coupling sweep + guard test

- Classify every remaining `/Users/` literal under `crates/` (real default vs test-data). Fix real defaults; allowlist justified test-data strings.
- Add the guard test that fails on new hardcoded `/Users/` literals outside the allowlist.
- Depth tests (in `crates/deadreckon/tests/portability_guard.rs`):
  - `no_hardcoded_user_paths_outside_allowlist`
  - `allowlist_entries_still_exist` (an allowlisted path that vanished is removed from the list)

### P8 — Dedicated test CI + restore the gate

- Add `.github/workflows/ci.yml`: on `push`/`pull_request`, run `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, and `cargo test --workspace` on `ubuntu-22.04`. No release secret. Keep Node-24 forward-compat in mind (pin `actions/*` versions).
- Restore `release-verify` in `release.yml` to run `cargo test --workspace` (now Linux-green); keep `cargo build --release --workspace --locked`.
- Depth test: `ci_workflow_runs_workspace_tests` — a YAML-shape assertion (parse `ci.yml`, assert a step invokes `cargo test --workspace`) in a small Node or Rust check, mirroring the existing release-plan static checks.

### P9 — Windows runtime smoke

- Add a CI job on `windows-latest` (in `ci.yml`, or a gated matrix leg) that builds the `x86_64-pc-windows-msvc` binary and runs `deadreckon --version` and `deadreckon doctor`, asserting exit 0. The job fails closed if the binary does not execute.
- Document in AS-BUILT which features are unix-only at runtime (sandbox backends, `nix` signal/process paths) so a green smoke is not mistaken for full Windows support.
- Depth test: `windows_smoke_job_runs_version_and_doctor` — YAML-shape assertion that the job invokes both commands.

### P10 — Windows signing fail-closed + runbook

- Confirm `release-trust.mjs` keeps `requires_windows_signing` stable-only and preflight blocks stable when `WINDOWS_CERT_PFX`/`WINDOWS_CERT_PWD` are absent. Add/keep a depth test that an official **stable** lane with missing Windows secrets fails preflight.
- Update `/Users/gdc/deadreckon/docs/RELEASE.md` with the secret-free Authenticode wiring checklist (export PFX, base64, set `WINDOWS_CERT_PFX`/`WINDOWS_CERT_PWD`, verify CI). No secret values.
- Depth tests (in the existing release-trust test module): `stable_windows_without_signing_secrets_fails_preflight`.

### P11 — Architecture doc update + CHANGELOG (doc only; no depth test)

- Insert a new top-level section into `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:
  ```
  ## NN. Release Trust Closure

  NN.1 CI topology (test CI vs release CI; what each gates)
  NN.2 Test-suite portability rules and the /Users guard
  NN.3 Windows posture (builds, smoke-tested, signing stable-only, unix-only gaps)
  NN.4 Repository identity (canonical gregce/deadreckon; re-home scope)
  ```
- If AS-BUILT has a "shipped vs thin" section, move "workspace tests run in CI" and "Windows binary executes (smoke)" to shipped; state plainly that full Windows runtime support and Authenticode-signed stable Windows remain deferred.
- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:
  ```
  ## Release Trust Closure (production-release track) — 2026-06-04

  - Workspace test suite runs in CI on Linux; release-verify gates on it again.
  - Windows binary smoke-tested on windows-latest; stable Windows stays signing-gated.
  - Completed gdc -> gregce repository re-home.
  ```
- Log any deferred decision (full Windows support, macOS CI matrix) in `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| `hardcoded /Users path in <file>:<line>` | `derive the path from DEADRECKON_HOME or the OS home; see paths.rs` |
| `pty test invoked `script` directly` | `use tests/support/pty.rs run_under_pty(...)` |
| `stable Windows release missing signing secrets` | `set WINDOWS_CERT_PFX and WINDOWS_CERT_PWD (docs/RELEASE.md)` |
| `windows smoke: deadreckon doctor exited non-zero` | `inspect the windows-latest job log; gate or fix the unix-only path` |

## Out of scope (explicitly not in this milestone)

- Full Windows feature parity (sandbox backends, signal handling on Windows). Document the gaps; do not implement.
- A macOS CI test matrix leg (Linux green is the bar for this slice; note macOS as a V1 candidate).
- Real Authenticode signing or any real publish by the agent.
- npm/Homebrew publish behavior changes (parent rider owns those lanes).
- Any `PipelineState` / plan / campaign / chain / receipt schema change.

## Dependencies (Tier 1 / 2 / 3 policy)

- Tier 1 (utility, free): a portable PTY test helper crate (e.g. `portable-pty` as a `dev-dependency`) is acceptable if a per-OS `script` dispatch proves fragile; justify in the commit.
- Tier 2 (architectural, log to `DEPENDENCIES.md`): none expected.
- Tier 3 (blocked): same blocks as prior riders — no new runtime provider SDKs, no networked test dependencies.

## Engineering invariants (do not violate)

- **No DeadReckon durable runtime schema changes.** Resolution logic may change; persisted shapes may not.
- **One depth test before each phase implementation.** A phase whose tests were never red is suspect.
- **No silent skips.** A test that cannot run on a platform is `ignore`d with a named reason, not deleted or hidden.
- **Fail-closed stays fail-closed.** Stable Windows without signing secrets must fail preflight; the smoke job must fail when the binary does not run.
- **No silent scope expansion.** Anything beyond P1–P11 goes to `V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push`, tag push, or real publish.
- Each phase ends with its depth tests passing and a CHANGELOG entry naming the SHA.
- If a phase reveals a V1-architecture decision (e.g. real Windows support), stop and log it in `V1-CANDIDATES.md`; do not expand scope.
