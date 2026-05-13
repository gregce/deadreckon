GOAL: Make deadreckon's run capabilities explicit, configurable, and sandbox-enforced. The Vercel deploy incident showed the current CLI-provider path can reuse host credentials, reach the network, run deploy tooling, and mutate global npm state while the user only asked for a coding run. Land a safer alpha default: coding runs still work unattended through subscription CLIs, but deploys, global installs, broad home access, and host credentials beyond the active provider require visible opt-in. Headline word: **Capability-safe**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - current provider/sandbox/config reality.
- `/Users/gdc/deadreckon/docs/goals/2026-05-13-1719-deadreckon-capability-config-rider.md` - exact config shape, tests, and phases.
- `/Users/gdc/deadreckon/docs/goals/2026-05-13-1345-deadreckon-provider-cli-ingest-rider.md` - descriptor/ingest invariants.
- Current seams: `crates/deadreckon-sandbox/src/{commands,policy,spec}.rs`, `crates/deadreckon-providers/src/{cli_common,registry,config,types}.rs`, `crates/deadreckon/src/{cli,main}.rs`.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` - invariants hold.

**Posture.** Stays `alpha`. No `PipelineState` schema changes. Config, provider descriptors, and per-run sidecar files may grow. No `git push`. Edits inside `/Users/gdc/deadreckon/`. True provider host allowlisting limits or remote deploy orchestration beyond Vercel-style CLI blocking go to `docs/V1-CANDIDATES.md`.

**Deliverables.**

- A capability model for runs: `coding` default, `networked`, `deploy`, and `unsafe-host` profiles.
- `~/.deadreckon/config.toml` gains clear permission defaults; each run writes `permissions.toml` next to `sandbox.toml`.
- `deadreckon init`, `run`, `extend`, `resume`, and `chain` preview the effective provider, sandbox, network, host credential roots, deploy/global-install policy, and blocked roots.
- CLI providers get only the active provider's credential/session roots by default. No broad `$HOME`, no `~/.vercel`, no `.npm-global` writes unless explicitly allowed.
- macOS Seatbelt and Linux bwrap enforce the same intent: deny non-allowlisted reads/writes, isolate or scope `HOME`, block network unless the capability profile permits it, and refuse deploy/global-install affordances with actionable `try:` lines.
- TUI and `status` show capability badges so a user can see whether a run is coding-safe, deploy-enabled, or unsafe-host.

**New user surface.**

- `deadreckon config permissions` - show or set default capability profile.
- `deadreckon run "goal" --profile coding|networked|deploy|unsafe-host`.
- `deadreckon run "goal" --allow deploy --allow host-credentials:vercel`.
- `deadreckon run "goal" --deny global-install`.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implement -> focused tests for touched crates -> conventional local commit -> CHANGELOG. P11 updates AS-BUILT §10/§11/§19/§22.

**Verification.**

- Every rider depth test is present and passing; `cargo fmt --check` and clippy for touched crates are green.
- Smoke: a fake Claude/Codex provider can complete a coding run but cannot run fake `vercel`, read `~/.vercel`, or write global npm state without `--allow deploy` / `--allow global-install`.
- Smoke: a deploy-profile run previews the elevated capabilities and records them in `permissions.toml`.
- Chain/extend/resume preserve and display the same capability policy.
- No edits outside `/Users/gdc/deadreckon/`. No `git push`. No `PipelineState` schema changes.

**Stop when** verification passes, AS-BUILT and CHANGELOG describe "Capability-safe config (alpha)", deferred host-network allowlisting details are in `docs/V1-CANDIDATES.md`, and the work is committed locally.
