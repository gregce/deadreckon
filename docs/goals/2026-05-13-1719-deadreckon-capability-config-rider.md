# deadreckon - Capability-Safe Config Rider

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-13-1719-deadreckon-capability-config-goal.md`.
It supersedes nothing in prior riders, especially provider registry and provider
CLI ingest. Their invariants still apply. This rider turns the current
provider/sandbox behavior into an explicit, user-visible, sandbox-enforced
capability model.

**All paths absolute.** Source `/Users/gdc/deadreckon/`; runtime
`/Users/gdc/.deadreckon/`; provider-owned host roots only when explicitly
allowlisted by profile.

## Posture (decided - do not redesign)

- **Maturity stays `alpha`.**
- **No `PipelineState` schema changes.** Persist new per-run permission state
  as files under the run root, not fields on `PipelineState`.
- **Do not remove subscription CLI support.** Claude/Codex/Gemini/OpenCode and
  descriptor-backed CLIs must still work unattended when configured.
- **Default is coding-safe, not deploy-safe.** A normal run can call its model
  provider and write the working tree, but cannot silently publish, use unrelated
  host credentials, or mutate global tools.
- **Provider-owned transcripts are read-only for ingest.** This work may allow
  the provider's own credential/session roots during execution, but must not
  rewrite provider logs outside the provider's normal operation.
- **No `git push`.** Phased local commits only.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Current Behavior To Capture In Tests

The rider exists because these are true today or were observed during run
`3823df13`:

- CLI providers set `ToolSandboxPolicy::cli_provider(...).allow_network = true`
  with `network_allowlist = ["*"]`.
- CLI providers pass the host `HOME` through and allow provider roots such as
  `~/.claude` / `~/.codex`.
- `resolve_cli_binary` can grant broad roots like `~/.npm-global`, making a
  provider able to run or mutate unrelated global CLIs when the binary lives
  there.
- macOS `sandbox-exec` profile begins with `(allow default)`, so file read/write
  restrictions are not acting as a deny-by-default filesystem boundary.
- A provider sub-agent was able to run `vercel`, upgrade global Vercel via
  `npm i -g vercel@latest`, switch Vercel team context, deploy to production,
  and write deployment metadata.

The implementation must replace this with explicit capabilities and tests that
would have caught the behavior.

## Config Additions

Config lives at `/Users/gdc/.deadreckon/config.toml`. Keep existing keys
compatible. Add a `[permissions]` table:

```toml
[permissions]
profile = "coding"              # coding | networked | deploy | unsafe-host
confirm_elevation = true
show_preview = true

[permissions.defaults]
network = "provider"            # none | provider | open
home = "provider-scoped"        # ephemeral | provider-scoped | host
deploy = "deny"                 # deny | ask | allow
global_install = "deny"         # deny | ask | allow
host_credentials = ["provider"] # provider, vercel, netlify, gh, npm, all

[permissions.provider_roots]
"cli:claude-code" = ["~/.claude"]
"cli:codex" = ["~/.codex"]
"cli:gemini" = ["~/.gemini"]
"cli:opencode" = ["~/.local/share/opencode"]
```

Profile semantics:

| Profile | Network | HOME | Deploy | Global install | Host credentials |
|---|---|---|---|---|---|
| `coding` | provider | provider-scoped | deny | deny | provider only |
| `networked` | open | provider-scoped | deny | deny | provider only |
| `deploy` | open | provider-scoped | allow | ask | provider + named deploy credentials |
| `unsafe-host` | open | host | allow | allow | all |

`unsafe-host` must always show a warning in preview, status, and TUI.

## Per-Run Sidecar

Write `/Users/gdc/.deadreckon/runstate/<scope>/runs/<run-id>/permissions.toml`
before the provider turn starts.

```toml
version = 1
profile = "coding"
network = "provider"
home = "provider-scoped"
deploy = "deny"
global_install = "deny"
host_credentials = ["provider"]
provider = "cli:claude-code"
provider_roots = ["~/.claude"]
read_allowlist = ["<resolved provider binary closure>", "<working_dir>", "~/.claude"]
write_allowlist = ["<working_dir>", "~/.claude"]
denied_roots = ["~/.ssh", "~/.vercel", "~/.netlify", "~/.npm-global", "~/.npmrc"]
warnings = []
```

Resume and extend must reuse the parent/run policy unless the user explicitly
supplies a new profile or `--allow`/`--deny` override.

## Capability Resolution

Inputs, highest precedence first:

1. CLI flags: `--profile`, repeated `--allow`, repeated `--deny`.
2. Chain step overrides.
3. Existing run `permissions.toml` for `resume` / default `extend`.
4. `~/.deadreckon/config.toml [permissions]`.
5. Built-in `coding`.

Capability names:

- `network`
- `deploy`
- `global-install`
- `host-credentials:<name>`
- `host-home`

Refusal examples:

| Attempt / state | Refusal |
|---|---|
| acceptance asks for public deployment but profile is `coding` | `try: deadreckon run "goal" --profile deploy` |
| provider tries to use Vercel credentials without opt-in | `try: deadreckon run "goal" --allow deploy --allow host-credentials:vercel` |
| global install is blocked | `try: rerun with --allow global-install, or add the tool to the project devDependencies` |
| sandbox backend resolves to `none` for a real provider | `try: deadreckon run "goal" --sandbox auto --profile coding` |

## Sandbox Enforcement

### macOS Seatbelt

Replace allow-by-default filesystem behavior with deny-by-default. The profile
must:

- deny file read/write by default;
- allow execution/read of the provider binary closure and system/runtime paths
  needed to start it;
- allow read/write of the working directory;
- allow only profile-resolved provider roots;
- explicitly deny sensitive roots (`~/.ssh`, unrelated credential roots,
  package-manager globals) before any broad allow;
- deny network unless policy says `provider` or `open`.

If host-level network domain allowlisting is not feasible with Seatbelt, model
`network = "provider"` and `network = "open"` the same at the OS layer but keep
the distinction in preview/docs and block deploy/credential/global-install
through filesystem and executable allowlists. Record that limitation in
`docs/V1-CANDIDATES.md`.

### Linux bwrap

Keep tmpfs `$HOME` by default. Bind only:

- working directory;
- provider binary closure;
- configured provider roots;
- explicitly allowed credential roots.

Do not bind `~/.npm-global`, `~/.vercel`, `~/.netlify`, or `~/.config/gh`
unless the capability profile names them.

### Docker

Keep Docker opt-in. It should receive the same permission projection: mounted
provider roots only when allowed, no host home by default, and network disabled
unless the resolved profile allows it.

## UX Contracts

Run/extend/chain preview must include:

```text
capabilities
  profile:  coding
  network:  provider
  home:     provider-scoped
  allowed:  provider credentials for cli:claude-code, working tree
  blocked:  deploy credentials, global installs, ~/.ssh
  elevate:  deadreckon run "goal" --profile deploy
```

`status` and `attach` must display compact badges:

- `safe coding`
- `networked`
- `deploy allowed`
- `unsafe host`

`doctor` must explain whether the current OS sandbox can enforce the selected
profile, and must be explicit when macOS network host allowlisting is advisory.

## Phases

Each phase: write the named depth tests first and watch them fail; implement;
run focused verification for touched crates; conventional local commit; append
one CHANGELOG line. Do not run the full workspace suite unless requested.

### P1 - Current-behavior regression harness

- Add fixture tests that simulate a provider trying to deploy, read deploy
  credentials, and write global npm state.
- Tests should be red against the current implementation.

Depth tests:

- `cli_provider_cannot_execute_unlisted_deploy_binary_by_default`
- `cli_provider_cannot_write_global_npm_state_by_default`
- `sandbox_exec_profile_is_not_allow_default_for_filesystem`

### P2 - Permission model and config parser

- Add core/provider-side types for profile resolution without touching
  `PipelineState`.
- Parse `[permissions]` while preserving existing config files.
- Add config print/set support.

Depth tests:

- `permissions_config_roundtrips_existing_config_without_loss`
- `permissions_profile_coding_resolves_provider_scoped_home`
- `permissions_unknown_capability_refuses_with_try_line`

### P3 - Per-run `permissions.toml`

- Write per-run permissions before the provider turn.
- Load it for `resume` and default `extend`.
- Keep it human-readable and include resolved roots.

Depth tests:

- `run_writes_permissions_sidecar_before_provider_turn`
- `resume_reuses_permissions_sidecar`
- `extend_inherits_parent_permissions_unless_overridden`

### P4 - Provider binary closure and root minimization

- Replace broad `~/.npm-global` / `~/.local` grants with a resolved executable
  closure sufficient for the chosen provider.
- Descriptor `sandbox_reads` / `sandbox_writes` become provider roots, not broad
  host capabilities.

Depth tests:

- `cli_binary_resolution_does_not_grant_sibling_global_bins`
- `descriptor_provider_roots_are_the_only_home_writes_in_coding_profile`
- `provider_root_override_adds_only_named_paths`

### P5 - macOS Seatbelt hardening

- Make the generated profile deny filesystem access by default.
- Preserve provider CLI execution on macOS.
- Deny sensitive roots explicitly.

Depth tests:

- `sandbox_exec_profile_denies_home_by_default`
- `sandbox_exec_blocks_vercel_credentials_without_deploy_profile`
- `sandbox_exec_allows_provider_session_root_only`

### P6 - bwrap and Docker projection parity

- Ensure bwrap and Docker get the same resolved permission roots.
- Keep tmpfs home unless `unsafe-host`.

Depth tests:

- `bwrap_uses_tmp_home_for_coding_profile`
- `bwrap_binds_deploy_credentials_only_when_allowed`
- `docker_omits_host_home_unless_unsafe_host`

### P7 - Run/extend/resume/chain integration

- Add `--profile`, `--allow`, and `--deny` consistently to run surfaces that
  invoke provider turns.
- Chain conductor must propagate policy to each step and record it.

Depth tests:

- `run_preview_lists_effective_capabilities`
- `extend_prints_same_capability_summary_as_run`
- `chain_step_inherits_chain_permission_profile`

### P8 - Acceptance-aware elevation prompts

- If acceptance criteria mention public deployment, external URL, Vercel,
  Netlify, Fly, Render, Railway, Docker push, package publish, or npm global
  install, preview must explain the required capability.
- Default behavior is refuse or ask, not silently elevate.

Depth tests:

- `public_deployment_acceptance_requires_deploy_profile`
- `global_install_acceptance_requires_global_install_allow`
- `noninteractive_deploy_acceptance_refuses_with_try_line`

### P9 - Doctor/status/TUI visibility

- Add capability diagnostics to `doctor`.
- Add compact capability badges to `status` and `attach`.
- TUI should show whether blocked capability events occurred.

Depth tests:

- `doctor_reports_capability_enforcement_for_current_os`
- `status_prints_permission_profile_and_blocked_roots`
- `attach_live_state_renders_capability_badges`

### P10 - Friendly errors and docs

- Normalize all permission failures through one formatter.
- Update README/HOWTO with normal coding, deploy, and unsafe-host examples.
- Keep help concise and discoverable.

Depth tests:

- `permission_error_footer_suggests_profile_or_allow_flag`
- `help_shows_profile_without_overwhelming_core_lifecycle`
- `howto_contains_deploy_opt_in_example`

### P11 - Architecture doc + CHANGELOG

- Update `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:
  - §10 Provider Model: provider roots and credential posture.
  - §11 Sandbox Model: deny-by-default filesystem and profile mapping.
  - §17 CLI Surface: new run/config flags.
  - §19 Configuration & BYOK: `[permissions]`.
  - §22 Built vs Scaffolding-Thin: close the deploy-safety gap honestly.
- Append CHANGELOG section:

```markdown
## Capability-safe config (alpha) - 2026-05-13

- Added explicit run capability profiles and sandbox-enforced host credential boundaries.
```

## Focused Verification Ladder

Use targeted checks during phases:

```zsh
cargo test -p deadreckon-sandbox <filter>
cargo test -p deadreckon-providers --test cli_providers <filter>
cargo test -p deadreckon --test codebase <filter>
cargo test -p deadreckon --test agentic_loop <filter>
cargo fmt --check
cargo clippy -p deadreckon-sandbox --tests -- -D warnings
cargo clippy -p deadreckon-providers --tests -- -D warnings
cargo clippy -p deadreckon --tests -- -D warnings
```

Final focused matrix:

```zsh
cargo test -p deadreckon-sandbox
cargo test -p deadreckon-providers --test cli_providers
cargo test -p deadreckon --test codebase permissions
cargo test -p deadreckon --test agentic_loop permissions
cargo fmt --check
cargo clippy -p deadreckon-sandbox --tests -- -D warnings
cargo clippy -p deadreckon-providers --tests -- -D warnings
cargo clippy -p deadreckon --tests -- -D warnings
```

Run full workspace verification only if requested or after the focused matrix
is green and runtime budget is acceptable.

## Error-Footer Canonical Pairs

| Error | `try:` |
|---|---|
| deploy blocked | `deadreckon run "goal" --profile deploy` |
| Vercel creds blocked | `deadreckon run "goal" --allow deploy --allow host-credentials:vercel` |
| global install blocked | `add the package to devDependencies, or rerun with --allow global-install` |
| sandbox none unsafe | `deadreckon run "goal" --sandbox auto --profile coding` |
| host home requested | `deadreckon run "goal" --profile unsafe-host` |

## Out of scope

- Building a remote deploy orchestrator.
- Reliable per-host network allowlisting for every provider CLI. If macOS
  Seatbelt cannot enforce domain-level provider-only networking, document it.
- New providers.
- New `PipelineState` fields.
- Rewriting provider-owned logs or credential stores.
- Automatically publishing to package registries.

## Dependencies

Tier 1 utility dependencies are allowed only if they materially simplify config
or shell parsing and have small transitive trees. Prefer existing crates:
`serde`, `toml`, `clap`, `tempfile`, `assert_cmd`, and `predicates`.

Tier 2 architectural dependencies are not expected. If executable-closure
resolution requires a new crate, log it in `DEPENDENCIES.md` and justify why
plain path resolution is insufficient.

Tier 3 blocked: daemon sandboxes, Docker SDKs, policy engines that require a
background service, or network libraries added only for host allowlisting.

## Engineering invariants

- No `PipelineState` schema changes.
- The profile shown in preview must be the policy enforced by the sandbox.
- Every permission refusal must include a `try:` line.
- Default coding profile must block deploy credentials and global installs.
- `unsafe-host` must never be implicit.
- Existing smoke provider and API providers must keep working.

## Process invariants

- Phased local commits only. No `git push`.
- Write depth tests before implementation in every P1-P10 phase.
- If an OS sandbox cannot enforce a stated invariant, change the preview text
  and document the limitation instead of pretending it is enforced.
- Major follow-ons go to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.
