# deadreckon — Distribute Rider (one-command install + channel-aware self-update)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-11-2343-deadreckon-distribute-goal.md`.
It supersedes nothing in prior riders (2026-05-10-build,
2026-05-11-{audit-harden, autonomous-chain, codebase, doc-depth, orchestrate,
overnight, primary-flow, provider-registry, robust, self-documenting,
usability}) — their invariants still apply. This rider adds the release
pipeline, the npm wrapper / per-platform package pattern, the in-binary
updater, and the install-receipt that lets `deadreckon update` route to the
right channel.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime `~/.deadreckon/`
(smoke runtime `/Users/gdc/deadreckon/.deadreckon-smoke`).

## Posture (decided — do not redesign)

- **Maturity stays `alpha`.** Workspace version remains `0.1.0`; the first
  tagged release this rider enables is `v0.2.0`, cut by the **user**, not the
  agent.
- **No `PipelineState` schema changes.** Install-channel state lives in
  `~/.deadreckon/install-receipt.json` (written at install time by every
  channel). Startup-check cache lives in `~/.deadreckon/update-check.json`.
  Backups for in-place swap live in `~/.deadreckon/update-backups/<ts>/`.
- **Tag-pushing to trigger CI is a user action.** The agent commits
  `Cargo.toml`, `.github/workflows/release.yml`, `dist-workspace.toml`,
  npm wrapper sources, and the new `update` subcommand — but never runs
  `git tag` or `git push`. The first real release is `git tag v0.2.0 &&
  git push --tags` from the user.
- **One binary set per release.** Every channel resolves to the same
  GitHub Release archives (`deadreckon-<target>.{tar.xz,zip}` +
  `.sha256`). The npm wrapper packages those same binaries — it does not
  re-build from source on install.
- **No postinstall network download on the npm channel.** The
  Claude-Code/esbuild `optionalDependencies` pattern is the spec.
  `dist`'s built-in npm installer (which postinstalls a download) is
  rejected for this reason; we layer `cargo-npm` on top or post-process
  dist's archives in the workflow.
- **No `git push` from the agent.** Phased local commits only.
- **No V1 invention.** Windows codesigning, MSI, App Store distribution,
  auto-rollback on failed update, delta updates, mirror registries,
  channel-aware telemetry — all V1; if a phase surfaces one, log to
  `docs/V1-CANDIDATES.md` and continue.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

### Overlap with peer riders — land non-conflicting

- **Overnight P10 (`--plain`, `--quiet`, lifecycle hints).** The update
  subcommand and the startup stale-check both route their output through
  `ui_card` if overnight has landed first; if overnight has not landed,
  use plain `println!` and convert in a follow-up — do not re-invent
  card primitives here.
- **Provider-registry.** This rider does not touch the provider layer.
  The `provider list` verb keeps its current shape.
- **Autonomous-chain.** Chain runs use whatever binary is on PATH; if
  `deadreckon update` swaps the binary mid-chain, the running chain
  continues with the old binary (process image stays mapped). A depth
  test pins this behavior.

## Data model (files, not fields)

### `~/.deadreckon/install-receipt.json`

Written once at install time by every channel. Read by `deadreckon
update` to route. If the file is missing on first `update` invocation,
deadreckon best-effort-detects the channel from `argv[0]` and writes a
receipt before proceeding.

```json
{
  "channel": "npm",
  "channel_version": "0.2.0",
  "binary_path": "/Users/gdc/.bun/install/global/node_modules/deadreckon/bin/deadreckon",
  "installed_at": "2026-05-11T23:55:12Z",
  "install_source": "https://registry.npmjs.org/deadreckon/-/deadreckon-0.2.0.tgz",
  "platform_package": "deadreckon-darwin-arm64",
  "receipt_version": 1
}
```

Field reference:

- `channel` — one of `"npm"`, `"brew"`, `"shell"`, `"cargo"`, `"source"`.
  `"shell"` covers both the `curl|sh` and `irm|iex` installers.
  `"source"` is the fallback when no other channel wrote a receipt
  (e.g., `cargo install --path`).
- `channel_version` — semver of the package as the channel sees it
  (npm package version, brew formula version, shell-installer version).
  May differ briefly from `Cargo.toml` during a release.
- `binary_path` — absolute path to the binary the channel installed.
  `axoupdater` uses this to swap on the `shell` channel; other channels
  use it only to confirm the running binary matches the receipt.
- `platform_package` — null on non-npm channels. On npm, the
  per-platform package name (`deadreckon-darwin-arm64`, etc.).
- `receipt_version` — schema version. Bump if fields change.

### `~/.deadreckon/update-check.json`

Background-check cache. Refreshed at most every 24h.

```json
{
  "checked_at": "2026-05-11T23:55:12Z",
  "latest_version": "0.2.3",
  "current_version": "0.2.0",
  "release_url": "https://github.com/gdc/deadreckon/releases/tag/v0.2.3",
  "is_stale": true
}
```

### `~/.deadreckon/update-backups/<ts>/`

Created only by `shell`-channel in-place swap. Contains `deadreckon`
(the prior binary) plus `receipt.json` (the prior receipt). Pruned to
the most recent three by every successful swap.

## npm wrapper layout (the spec)

The wrapper package `deadreckon` and the five per-platform packages
ship from one CI workflow.

### Wrapper `package.json`

```json
{
  "name": "deadreckon",
  "version": "0.2.0",
  "description": "Long-running, BYOK, sandboxed agentic CLI harness.",
  "bin": { "deadreckon": "bin/deadreckon.js" },
  "optionalDependencies": {
    "deadreckon-darwin-arm64": "0.2.0",
    "deadreckon-darwin-x64":   "0.2.0",
    "deadreckon-linux-x64":    "0.2.0",
    "deadreckon-linux-arm64":  "0.2.0",
    "deadreckon-win32-x64":    "0.2.0"
  },
  "scripts": { "postinstall": "node bin/postinstall.js" },
  "files": ["bin/"],
  "engines": { "node": ">=18" }
}
```

The `postinstall` writes `~/.deadreckon/install-receipt.json` and
nothing else — **no network, no archive download**. If the matching
optional dependency is missing (host CPU/OS unsupported), postinstall
exits 0 with a `try:` hint pointing at the shell installer.

### Platform `package.json` (one per target)

```json
{
  "name": "deadreckon-darwin-arm64",
  "version": "0.2.0",
  "os": ["darwin"],
  "cpu": ["arm64"],
  "bin": { "deadreckon": "bin/deadreckon" },
  "files": ["bin/"]
}
```

The `bin/deadreckon` inside each platform package is the prebuilt
binary verbatim, mode `0755`. No wrapper script.

### Wrapper `bin/deadreckon.js`

Resolves the platform package and execs the binary with the inherited
argv. Single source of truth:

```js
#!/usr/bin/env node
const { execFileSync } = require("child_process");
const path = require("path");
const target = `deadreckon-${process.platform}-${process.arch === "x64" ? "x64" : process.arch}`;
const binPath = require.resolve(`${target}/bin/deadreckon`);
try {
  execFileSync(binPath, process.argv.slice(2), { stdio: "inherit" });
} catch (e) {
  process.exit(e.status ?? 1);
}
```

(Windows: append `.exe` to `binPath`. Handled by the postinstall step
when `process.platform === "win32"`.)

## Release pipeline (`dist`)

`dist-workspace.toml` (committed at repo root) declares the matrix and
the installers:

```toml
[dist]
cargo-dist-version = "0.31.0"
ci = ["github"]
installers = ["shell", "powershell", "homebrew"]
targets = [
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "x86_64-unknown-linux-gnu",
  "aarch64-unknown-linux-gnu",
  "x86_64-pc-windows-msvc",
]
linux-zigbuild = true
linux-glibc = "2.28"
tap = "gdc/homebrew-tap"
allow-dirty = ["ci"]
publish-jobs = ["./publish-npm"]
```

The npm publish is a separate workflow step (`./publish-npm`) — *not*
dist's bundled npm installer. That step:

1. Downloads the dist-built archives from the GitHub Release.
2. For each target, unpacks `bin/deadreckon[.exe]` into
   `npm/deadreckon-<plat>-<arch>/bin/`.
3. Writes the per-platform `package.json` from a template.
4. `npm publish --access public` for each platform package.
5. Updates the wrapper `package.json` to reference the new version
   and `npm publish` it last.

## macOS signing + notarization

In the dist-generated workflow:

```yaml
- name: Codesign macOS binary
  env:
    APPLE_CERT_P12:    ${{ secrets.APPLE_CERT_P12 }}
    APPLE_CERT_PWD:    ${{ secrets.APPLE_CERT_PWD }}
    APPLE_ID:          ${{ secrets.APPLE_ID }}
    APPLE_TEAM_ID:     ${{ secrets.APPLE_TEAM_ID }}
    APPLE_APP_PWD:     ${{ secrets.APPLE_APP_PWD }}
  run: |
    echo "$APPLE_CERT_P12" | base64 -d > /tmp/cert.p12
    security create-keychain -p "" build.keychain
    security import /tmp/cert.p12 -k build.keychain -P "$APPLE_CERT_PWD" -T /usr/bin/codesign
    codesign --sign "Developer ID Application" --options runtime --timestamp ./deadreckon
    ditto -c -k --keepParent ./deadreckon ./deadreckon.zip
    xcrun notarytool submit ./deadreckon.zip \
      --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PWD" --wait
```

If the cert secret is absent (forks, local dry-run), the step skips
with a warning; unsigned macOS binaries are clearly tagged in the
release notes.

## Verb signatures

```
deadreckon update
    [--check]                # non-mutating; print latest version + channel hint and exit
    [--force]                # skip same-version short-circuit (still respects channel routing)
    [--allow-prerelease]     # consider prereleases when computing "latest"
    [--quiet]                # suppress lifecycle hint; print only fatal errors
```

Refusal cases:

| Case | Behavior |
|---|---|
| Channel = `source`, no `--force` | Exit 1 with `try: cargo install --path crates/deadreckon` |
| Channel = `npm`/`brew`/`cargo` | Exit 0 after printing channel-native upgrade command with `try:` |
| Channel = `shell`, swap fails | Exit 2, leave prior binary in place, point at backup dir |
| Receipt missing, channel undetectable | Exit 3 with `try: deadreckon doctor` |
| Network unreachable | Exit 4 with `try: deadreckon update --check` once online |

## Channel detection (when receipt missing)

Pseudocode for the first-time detection:

```
binary = canonicalize(argv[0])
if binary contains "node_modules/deadreckon/":         channel = "npm"
elif binary contains "/Cellar/" or under brew prefix:   channel = "brew"
elif binary under ~/.cargo/bin/:                        channel = "cargo"
elif binary under ~/.local/share/deadreckon/ or
     under %LOCALAPPDATA%\deadreckon\:                  channel = "shell"
else:                                                   channel = "source"
```

After detection, write a receipt so subsequent runs are O(1).

## Startup stale-check

Runs as a `tokio::spawn` in `main.rs` before any blocking work, with a
50ms budget on the disk read of `update-check.json`. If the cache is
within 24h and `is_stale` is true, print one line on stderr after the
top-level command exits:

```
→ deadreckon 0.2.3 is available. Run `deadreckon update`.
```

Disabled when:

- `DEADRECKON_UPDATE_CHECK=0` is set.
- stdout is not a TTY (CI, pipes).
- the receipt's `channel` is `source`.
- the subcommand is `update` itself or `doctor`.

The network refresh of the cache runs only when the cache is missing
or older than 24h, in a detached task with a 3s timeout; failures are
silent.

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them
fail; implement; green on `cargo build --release && cargo test --workspace
&& cargo clippy --workspace -- -D warnings && cargo fmt --check`;
conventional-commit local commit; one-line CHANGELOG entry.

### P1 — Receipt + cache plumbing

- Add `crates/deadreckon-core/src/install_receipt.rs` with `Receipt`,
  `Channel`, `read()`, `write()`, `detect()`. No behavior change yet.
- Add `crates/deadreckon-core/src/update_cache.rs` with `Cache`,
  `read()`, `write()`, `is_stale()`.

Depth tests (in `crates/deadreckon-core/tests/`):

- `install_receipt_roundtrips_every_channel_variant`
- `install_receipt_detects_npm_path_layout`
- `install_receipt_detects_brew_cellar_layout`
- `install_receipt_detects_cargo_bin_layout`
- `install_receipt_detects_shell_install_layout`
- `install_receipt_falls_back_to_source_on_unknown_path`
- `update_cache_is_stale_after_24h`
- `update_cache_round_trips_release_url`

### P2 — `deadreckon update --check`

- Add the `update` subcommand to clap in `crates/deadreckon/src/main.rs`.
- Implement `--check` only: prints `channel: <name>` and `current:
  <ver>` and `latest: <ver>` (querying GitHub Releases). Exits 0.
- Does not write files except refreshing `update-check.json`.

Depth tests:

- `update_check_prints_channel_from_receipt`
- `update_check_exits_zero_when_no_network`
- `update_check_refreshes_cache_when_stale`
- `update_check_does_not_write_receipt`

### P3 — `update` channel routing (non-shell)

- For `npm`/`brew`/`cargo`: print the channel-native upgrade command
  with `try:` and exit 0. For `source`: refuse with the documented
  `try:`. No network needed.
- Wire one error-footer canonical pair per channel.

Depth tests:

- `update_npm_prints_bun_update_hint`
- `update_brew_prints_brew_upgrade_hint`
- `update_cargo_prints_binstall_or_install_hint`
- `update_source_refuses_with_cargo_install_path`

### P4 — `update` shell-channel binary swap

- Embed `axoupdater` as a workspace dep, behind a `selfupdate` feature
  flag (default-on). Swap the running binary via
  `axoupdater::AxoUpdater::run`. Back up the prior binary to
  `~/.deadreckon/update-backups/<ts>/`. Prune to the most recent three.
- On swap failure: leave the prior binary in place, exit 2, print the
  backup directory path.

Depth tests:

- `update_shell_writes_backup_before_swap`
- `update_shell_prunes_backups_to_three`
- `update_shell_swap_failure_preserves_binary`
- `update_shell_rejects_swap_on_non_shell_receipt`

### P5 — Startup stale-check

- Spawn the check from `main.rs` before subcommand dispatch. Honor
  every disable rule listed above. Print the single-line hint after
  the subcommand returns, on stderr.
- The network refresh path runs detached with a 3s timeout; never
  blocks the subcommand.

Depth tests:

- `startup_check_skips_under_non_tty`
- `startup_check_skips_under_env_disable`
- `startup_check_skips_for_source_channel`
- `startup_check_does_not_block_subcommand_exit`
- `startup_check_prints_hint_when_cache_stale`

### P6 — `dist-workspace.toml` + GitHub Actions release workflow

- Run `dist init` interactively once, commit the generated
  `dist-workspace.toml` and `.github/workflows/release.yml`. Adjust
  targets to the five listed in the goal. Enable `linux-zigbuild` and
  `linux-glibc = "2.28"`. Disable dist's bundled npm installer
  (we layer our own in P8).
- Add `dist plan --output-format=json` as a CI check on every push.

Depth tests (in `tests/release_plan.rs`, using `dist plan` if
installed, skipped otherwise):

- `dist_plan_lists_all_five_targets`
- `dist_plan_pins_linux_glibc_2_28`
- `dist_plan_excludes_bundled_npm_installer`

### P7 — macOS codesign + notarize step

- Append the codesign + notarytool step to the generated workflow
  inside an `if: matrix.target == '*-apple-darwin'` block. Skip the
  step when `APPLE_CERT_P12` is absent (forks, dry-runs).
- Document the five required secrets in `docs/RELEASE.md` (new file).

Depth tests:

- `release_workflow_codesigns_only_on_apple_targets`
- `release_workflow_skips_codesign_without_cert_secret`
- `release_doc_lists_all_five_apple_secrets`

### P8 — npm publish workflow + per-platform packages

- Add `npm/` directory with one template per platform package and
  the wrapper. Add `.github/workflows/publish-npm.yml` (called from
  the release workflow after dist's job succeeds):
  1. Downloads the dist archives via `gh release download`.
  2. Repacks `bin/deadreckon[.exe]` into each platform package dir.
  3. Writes per-platform `package.json` from template.
  4. `npm publish --access public` per package, then wrapper.
- Wrapper postinstall writes the install receipt; no network.

Depth tests (`tests/npm_wrapper.rs`):

- `npm_wrapper_optional_deps_match_five_platforms`
- `npm_wrapper_postinstall_writes_receipt_no_network`
- `npm_wrapper_bin_resolves_to_platform_package`
- `npm_platform_package_contains_single_executable`
- `npm_platform_package_json_pins_os_and_cpu`

### P9 — Homebrew tap publish

- Enable `tap = "gdc/homebrew-tap"` in `dist-workspace.toml`. dist
  generates the formula and pushes to the tap repo (separate from
  this one).
- Formula's `install` writes the install receipt with channel `"brew"`.

Depth tests:

- `homebrew_formula_writes_install_receipt`
- `homebrew_formula_pins_release_sha256`

### P10 — Friendliness pass

- Auto-detect channel on first `update` if receipt missing; write a
  receipt before proceeding.
- Preview before any shell-channel swap: print target version, archive
  URL, sha256, backup path; require `--yes` under non-TTY (default to
  prompt under TTY).
- Refuse with `try:` on every documented error case.
- Lifecycle hint after a successful update: `try: deadreckon doctor`.
- Wire `--quiet` and `--plain` consistently with overnight P10 if it
  has landed; otherwise plain `println!` placeholders that the
  overnight rider will convert.

Depth tests:

- `update_writes_receipt_when_missing_on_first_run`
- `update_shell_previews_before_swap`
- `update_shell_requires_yes_under_non_tty`
- `update_success_prints_doctor_hint`
- `update_quiet_suppresses_lifecycle_hint`
- `update_plain_strips_color`

### P11 — Architecture doc update + CHANGELOG (doc only; no depth test)

- Insert a new top-level section into
  `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:

  ```
  ## 28. Distribution & Self-Update

  28.1 Channels (npm wrapper, shell/powershell installers, Homebrew tap, cargo, source)
  28.2 The install receipt (`~/.deadreckon/install-receipt.json`)
  28.3 The `deadreckon update` verb and channel routing
  28.4 Startup stale-check (cache, disable rules, non-blocking)
  28.5 Release pipeline (`dist`, glibc-pinned zigbuild, macOS codesign+notarize)
  28.6 npm wrapper layout (`optionalDependencies`, per-platform packages, no postinstall download)
  ```

- Update `§22 What's Built vs Scaffolding-Thin`:
  - Add to **shipped**: prebuilt cross-platform binaries; npm wrapper;
    shell/powershell/Homebrew installers; channel-aware `update`;
    startup stale-check.
  - Note: this rider opens new ground (distribution) rather than
    closing prior thin items. No prior §22 thin row is removed.

- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:

  ```
  ## Distribution & self-update (alpha) — 2026-05-11

  - Added `dist`-driven release pipeline producing signed/notarized binaries for macOS arm64/x64, glibc-2.28-pinned Linux x64/arm64, and Windows x64.
  - Added the npm wrapper (`bun install -g deadreckon`) using the `optionalDependencies` per-platform pattern with no postinstall network download.
  - Added shell/powershell one-line installers and a Homebrew tap, all writing `~/.deadreckon/install-receipt.json` at install time.
  - Added the `deadreckon update` subcommand with channel-aware routing (npm/brew/cargo print channel-native upgrade hints; shell does an in-place swap with backups; source refuses with a `try:`).
  - Added a non-blocking startup stale-version check, cached 24h at `~/.deadreckon/update-check.json`, disabled under non-TTY / `DEADRECKON_UPDATE_CHECK=0` / source channel.
  - Updated `docs/AS-BUILT-ARCHITECTURE.md` with `§28 Distribution & Self-Update` and amended `§22`.
  ```

- Optional demo: `docs/assets/casts/distribute.cast` capturing the
  three install commands and a `deadreckon update --check`. Skip if
  the binary swap isn't testable from inside this repo.

## Integration matrix (channels × actions)

| Channel | Install                                             | Update behavior                              | Refusal cases handled                |
|---------|-----------------------------------------------------|----------------------------------------------|--------------------------------------|
| npm     | `bun install -g deadreckon`                         | Print `bun update -g deadreckon` hint        | host CPU/OS unsupported              |
| shell   | `curl -LsSf …/installer.sh \| sh` / PowerShell      | In-place swap via `axoupdater` + backup      | swap fails, network unreachable      |
| brew    | `brew install gdc/tap/deadreckon`                   | Print `brew upgrade gdc/tap/deadreckon` hint | tap not added (formula install hint) |
| cargo   | `cargo binstall deadreckon` or `cargo install`      | Print `cargo binstall --force` hint          | not on PATH                          |
| source  | `cargo install --path crates/deadreckon`            | Refuse with `try: cargo install --path …`    | receipt missing                      |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| `update: channel = source; in-place swap not supported` | `cargo install --path crates/deadreckon` |
| `update: receipt missing; cannot detect channel` | `deadreckon doctor` |
| `update: swap failed; prior binary preserved` | `cp ~/.deadreckon/update-backups/<ts>/deadreckon $(command -v deadreckon)` |
| `update: network unreachable` | `deadreckon update --check  (when online)` |
| `update: same version (use --force to re-run)` | `deadreckon update --force` |
| `bun install -g deadreckon` fails on unsupported host | `curl -LsSf https://…/deadreckon-installer.sh \| sh` |

(Each pair is parameterized over a depth test in P3, P4, or P10.)

## Config additions

```toml
[defaults]
update_check = "auto"   # "auto" | "on" | "off"  (env override: DEADRECKON_UPDATE_CHECK)
update_channel_override = ""  # rarely needed; pin channel when receipt is wrong
```

## Out of scope (V1 candidates — log if surfaced)

- Windows codesigning (Azure Trusted Signing or EV cert).
- Microsoft Store / winget / Chocolatey publishing.
- Apple Mac App Store distribution.
- Delta / patch updates (currently each update downloads the full archive).
- Auto-rollback when a newly-swapped binary segfaults on startup.
- Channel-aware telemetry on update outcomes.
- Mirror registry support for air-gapped installs.
- Signature verification chain for the shell installer (curl-bash is
  trusted via TLS only in this milestone).
- A `deadreckon uninstall` verb.
- Linux musl variants (`-unknown-linux-musl`).
- Prerelease channels (`--allow-prerelease` exists but no prerelease
  publishing flow yet).

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (utility, free):

- `axoupdater` — embedded updater for the shell channel. Already used
  by ~every dist-shipped Rust CLI.
- `semver` — version comparison for the stale-check (already pulled in
  transitively; promote to direct).

Tier 2 (architectural, log to `DEPENDENCIES.md`):

- `cargo-dist` (build-time only, not a runtime dep) — release pipeline.
- `cargo-zigbuild` (build-time only, CI side) — glibc-pinned Linux
  cross-compile.

Tier 3 (blocked): same blocks as prior riders.

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes.** All new state lives in files
  at `~/.deadreckon/`.
- **One depth test before each phase implementation.** A phase whose
  tests were never red is suspect.
- **The npm wrapper does no postinstall network download.** This is
  the load-bearing invariant for surviving `--ignore-scripts`,
  offline caches, and corporate proxies. A depth test asserts the
  postinstall script contains no `https://` / `http://` literal and
  invokes no `fetch`/`https`/`curl`/`wget`.
- **`deadreckon update` never `git push`es and never executes
  `cargo install` itself.** It prints hints; the user runs them.
- **One binary set per release.** Platform packages contain
  byte-identical binaries to the GitHub Release archives. A depth
  test sha256-compares.
- **The startup stale-check is non-blocking.** Subcommand exit time
  must not depend on its progress. A depth test races the check
  against a fast subcommand and asserts the check is detached.
- **No silent expansion.** Anything beyond P1–P11 goes into
  `V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push` from the agent.
- Each phase ends with the relevant depth tests passing and a
  CHANGELOG entry naming the SHA.
- After P11, optionally capture an asciinema cast under
  `docs/assets/casts/distribute.cast`. Skip when the swap isn't
  testable from inside this repo.
- If a phase reveals a V1-architecture decision (Windows codesigning
  flow, delta updates, telemetry), stop and log it in
  `V1-CANDIDATES.md`; do not silently expand scope.
- The first real release (`git tag v0.2.0 && git push --tags`) is the
  user's action, not the agent's. The agent's deliverable is the
  green CI workflow on a dry-run tag (`git tag v0.2.0-dryrun-1`) only
  if the user explicitly requests it; otherwise the workflow stays
  untriggered.
