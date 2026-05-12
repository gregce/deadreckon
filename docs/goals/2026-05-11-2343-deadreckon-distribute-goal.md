GOAL: Make deadreckon installable in one command on every common OS and self-updatable regardless of install channel. Today it builds only from source, excluding every user who isn't a Rust developer. This goal lands prebuilt binaries for macOS (arm64+x64), Linux (x64+arm64, glibc-pinned), Windows (x64); an npm wrapper using the `optionalDependencies` per-platform pattern so `bun install -g deadreckon` works identically on all three OSes; a `curl|sh` + PowerShell `irm|iex` pair for users without a Node runtime; a Homebrew tap; and a `deadreckon update` subcommand that detects how the user installed and updates through the matching channel. Headline word: **Shippable**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` — §17, §19, §22.
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-2343-deadreckon-distribute-rider.md` — release matrix, npm schema, signing, receipt, depth tests.
- `https://github.com/axodotdev/cargo-dist` — `dist` v0.31+ pipeline.
- `https://github.com/axodotdev/axoupdater` — embedded updater + receipt format.
- `https://github.com/abemedia/cargo-npm` — `optionalDependencies` generator.
- `https://www.npmjs.com/package/@anthropic-ai/claude-code` — wrapper shape to mirror.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` — invariants hold.

**Posture.** Stays `alpha`. No `PipelineState` schema changes — install-channel state lives in `~/.deadreckon/install-receipt.json` plus a `~/.deadreckon/update-check.json` cache. Tag-pushing CI is a user action, never an agent action. No `git push` from the agent. Edits inside `/Users/gdc/deadreckon/`. V1 → `docs/V1-CANDIDATES.md`.

**Three channels, one binary set (schemas in rider).**

- **`bun install -g deadreckon`** — wrapper + per-platform packages (`deadreckon-{darwin-arm64, darwin-x64, linux-x64, linux-arm64, win32-x64}`) as `optionalDependencies` with `os`/`cpu` filters. **No postinstall download** — survives `--ignore-scripts`, offline caches, proxies.
- **`curl -LsSf …/deadreckon-installer.sh | sh`** / **`irm …/deadreckon-installer.ps1 | iex`** — zero-runtime, by `dist`.
- **`brew install gdc/tap/deadreckon`** — generated tap; `cargo binstall deadreckon` free.

**`deadreckon update`.** Embeds `axoupdater`. Reads `install-receipt.json` (channel: `npm` | `brew` | `shell` | `cargo` | `source`) and routes — npm/brew/cargo print the channel-native upgrade with `try:`; shell does an in-place binary swap with a backup at `~/.deadreckon/update-backups/<ts>/`; source refuses with `try: cargo install --path crates/deadreckon`. `--check` is non-mutating. Background check on startup (24h-cached) prints one stale-version hint; off under `DEADRECKON_UPDATE_CHECK=0` or non-TTY.

**Cross-compile + sign.** Linux via `cargo-zigbuild` pinned glibc 2.28. macOS via Developer ID + `codesign` + `notarytool submit --wait`; creds as repo secrets. Windows unsigned this milestone (V1).

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check` green → conventional-commit → CHANGELOG. P11 adds §28 "Distribution & Self-Update" to AS-BUILT, updates §22.

**Verification.**

- Commands green every commit; every rider depth test present and passing.
- npm-wrapper smoke: `npm pack` on the wrapper emits `optionalDependencies` matching the five platform packages; each platform tarball contains one `bin/deadreckon`.
- Updater smoke: with a synthetic receipt for each of `{npm, brew, shell, cargo, source}`, `deadreckon update --check` prints the correct channel hint and exits 0 without writing files.
- No edits outside `/Users/gdc/deadreckon/`. No `git push` from the agent.

**Stop when** verification passes, AS-BUILT updated, CHANGELOG has a "Distribution & self-update (alpha)" section, committed locally. First real release is the user's `git tag v0.2.0 && git push --tags`.
