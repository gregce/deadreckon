# deadreckon — Stable Readiness Rider (the last mile to v0.1.0)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-06-10-1628-deadreckon-stable-readiness-goal.md`.
It supersedes nothing in prior riders (`2026-06-05-0009-deadreckon-uniform-surface-rider.md`,
`2026-06-04-1951-deadreckon-release-trust-closure-rider.md`,
`2026-05-27-1152-deadreckon-production-command-model-rider.md`, and earlier) —
their invariants still apply, with one explicit exception: the Uniform Surface
rider's Tier-3 block on prompt crates is superseded by the shipped inquire
engine (AS-BUILT §42, DEPENDENCIES.md row `inquire 0.9.4`); that decision is
settled and must not be re-litigated in either direction.
This rider adds: populated model catalogs and a model-selection contract across
every launch surface, never-dead-end TTY rescue at provider refusal sites, the
two remaining walk-away durability fixes, embedded artifact checksums, a
real-provider proof harness, and the stable-lane operator checklist.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime `~/.deadreckon`
(env `DEADRECKON_HOME`; never hardcode a developer home — `tests/portability.rs`
guards this and the `gdc/` repo slugs).

## Posture (decided — do not redesign)

- **Maturity stays production-release track**; this rider is the gate between
  rc.N and stable v0.1.0.
- **No `PipelineState` schema changes.** Durable run state is frozen.
- **Additive serde-default fields only** on `plan.json` (`PlanProviders`) and
  provider descriptor TOMLs (`ModelEntry`). Old files must deserialize
  unchanged; new fields must default cleanly.
- **One new verb: `models`.** No other CLI verb additions.
- **No `git push`, no `git tag`, no release publication.** Operator actions
  (tap repo creation, npm trusted publishing, Windows secrets, tagging) are
  produced as a checklist in `docs/RELEASE.md`, never executed by the agent.
- **The inquire prompt engine is the only interactive surface.** New prompts go
  through `prompt::select_one` / `confirm` / `open`; line mode
  (`DEADRECKON_PROMPT_LINE_MODE`, off-TTY) stays byte-stable; PTY test
  harnesses pin line mode and send `\r`.
- **Byte-exact output tests are the contract.** Update goldens deliberately
  (`DEADRECKON_UPDATE_GOLDENS=1` exists for characterization), never loosen.
- **No V1 invention.** Architectural decisions surfaced mid-phase go to
  `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`; continue the phase.
- **Edits stay inside `/Users/gdc/deadreckon`.**

## Data model (files, not fields)

### Descriptor TOML: `[[model_catalog]]` entries (additive)

`crates/deadreckon-providers/src/registry/mod.rs` `ModelEntry` gains one
optional field (serde-default, additive):

```rust
pub struct ModelEntry {
    pub id: String,
    pub context_window: Option<u32>,
    pub input_per_million: Option<f64>,
    pub output_per_million: Option<f64>,
    pub aliases: Vec<String>,
    #[serde(default)]
    pub recommended: bool,   // NEW: exactly one per descriptor catalog
}
```

Catalog shape per CLI descriptor (example, `cli-claude-code.toml`):

```toml
[[model_catalog]]
id = "provider default"
context_window = 200000
aliases = ["claude-default"]
recommended = true

[[model_catalog]]
id = "opus"
context_window = 200000
aliases = ["claude-opus"]

[[model_catalog]]
id = "sonnet"
context_window = 200000
aliases = ["claude-sonnet"]

[[model_catalog]]
id = "haiku"
context_window = 200000
aliases = ["claude-haiku"]
```

Model ids for CLI providers are the strings the CLI's own `--model` flag
accepts (claude accepts `opus`/`sonnet`/`haiku` aliases; codex accepts
`gpt-5.1-codex` family ids; verify each CLI's accepted spellings against its
`--help` during P1 and record them in the descriptor — the descriptor is the
source of truth, not this rider's examples). `"provider default"` stays the
first, recommended entry everywhere: selecting it must result in NO `--model`
argument being passed to the CLI subprocess (existing behavior).

### `plan.json`: per-role models (additive, serde-default)

`crates/deadreckon-core/src/plan.rs` `PlanProviders` gains optional model
fields mirroring the existing route fields:

```rust
pub struct PlanProviders {
    pub planner: Option<String>,
    pub default_child: Option<String>,
    pub coder: Option<String>,
    pub reviewer: Option<String>,
    #[serde(default)]
    pub children: BTreeMap<u32, String>,
    // NEW (all serde-default):
    #[serde(default)]
    pub planner_model: Option<String>,
    #[serde(default)]
    pub default_child_model: Option<String>,
    #[serde(default)]
    pub coder_model: Option<String>,
    #[serde(default)]
    pub reviewer_model: Option<String>,
    #[serde(default)]
    pub child_models: BTreeMap<u32, String>,
}
```

Old `plan.json` files deserialize with every new field `None`/empty — pin with
a depth test that parses a pre-rider fixture verbatim.

### Known-good provider versions: `release/known-good-providers.json`

Written by P9's proof harness, committed, and referenced by release notes:

```json
{
  "schema_version": 1,
  "recorded_at": "<ISO8601>",
  "providers": [
    {
      "route": "cli:claude-code",
      "binary_version": "2.1.172 (Claude Code)",
      "proof": "start -> 2+ real turns -> gate signed -> apply -> kill/resume",
      "run_id": "<id>",
      "operator": "<who ran it>"
    }
  ]
}
```

## Model resolution (one rule, everywhere)

Per role, the resolved model is the first of:

1. explicit per-role flag (`--planner-model`, `--child-model IDX=`, …)
2. explicit `--model` (single-run surfaces: run/start/chain)
3. configured default (`config model X` → `defaults.model`)
4. descriptor catalog `recommended` entry
5. `"provider default"`

`"provider default"` resolves to *omitting* the model argument. The resolved
(provider, model) pair is computed in `setup.rs` selection (which already
carries `model` on `ProviderSetupSelection`) and must be displayed anywhere
the provider is displayed: start preview provider-roles table, run preview,
chain/orchestrate/campaign previews, verdict surfaces, `providers list`.
A model not present in the catalog is a warning, not a refusal (CLIs add
models faster than descriptors update): `warning: model X not in the
cli:claude-code catalog; passing it through`.

## Verb signatures

```
deadreckon models [PROVIDER]
    [--all]      # include providers without credentials
    [--json]     # additive JSON: {provider, models: [{id, recommended,
                 #   context_window, default}], configured_default}
```

Refusals:

| case | behavior |
|---|---|
| unknown PROVIDER | blocked verdict, `try: deadreckon providers list --all` |
| no providers configured at all | blocked verdict, `try: deadreckon init` |

Flag additions (no new verbs):

```
deadreckon orchestrate full-plan ... [--planner-model M] [--coder-model M]
    [--reviewer-model M] [--child-model IDX=M ...]
deadreckon orchestrate review   ... [--coder-model M] [--reviewer-model M]
deadreckon campaign run         ... [--planner-model M] [--child-model M]
```

`--child-model IDX=M` parses exactly like the existing
`--child-provider IDX=PROVIDER` (reuse its parser; same refusal copy for a
malformed pair).

## Never-dead-end rescue (P5 spec)

In `crates/deadreckon/src/setup.rs`, every `require_usable_route` refusal
(`SetupRefusal`) gains a TTY rescue: when `prompt::is_interactive()` and the
refusal is credential/login-shaped, render the probe-before-ask provider
picker (the `prompt_provider` menu from init, extracted to be callable from
setup) titled with the failure:

```
? provider anthropic needs credentials — pick another route
› Claude Code (cli:claude-code) — installed, logged in
  Codex (cli:codex) — installed, logged in
  ...
  Keep anthropic anyway — export ANTHROPIC_API_KEY first
  Cancel
```

- Selecting a route re-runs selection with that explicit provider and
  continues the launch (one rescue per launch; a second failure refuses as
  today — no loops).
- "Keep anyway" and "Cancel" produce today's refusal verbatim (the `try:`
  contract is unchanged for scripts, line mode, and non-TTY).
- Applies to: primary run/start selection, doc-provider selection, def-done
  provider selection. The rescue NEVER fires off-TTY, under
  `DEADRECKON_PROMPT_LINE_MODE`, with `--json`, `--plain`, `--quiet`, or
  `--no-confirm`/`--yes`.
- The chosen rescue route is for THIS launch only; print a hint:
  `make it the default: deadreckon config provider cli:claude-code`.

## Durability fixes (P6/P7 spec)

- **history.json**: `load_or_reconstruct_history` (turn_loop.rs ~1318)
  currently falls back to trace reconstruction only when the file is missing.
  Change: any read or parse failure logs one warning line
  (`history.json unreadable (<err>); reconstructing from traces.jsonl`) and
  takes the same reconstruction path; the reconstructed history is then
  re-saved atomically. Convert `save_history` to the tempfile+rename pattern
  `state.rs:445` uses.
- **lock reclaim**: `lock_is_stale` (lock.rs:196) is
  `age > stale_after || !pid_is_alive(...)`. Change to: a dead pid is always
  stale; an ALIVE pid is never reclaimable regardless of heartbeat age —
  instead the acquirer refuses with the existing LockHeld surface plus
  `try: deadreckon kill <run> --force` and a note naming the stale heartbeat
  age. Update the lock.rs doc comment that promises 30-minute usurpation.

## Stable-lane gates (P8 spec)

- **Embedded checksums**: set `checksum = "sha256"` semantics for cargo-dist
  (investigate the exact dist-workspace.toml key for v0.31; it already emits
  `*.sha256` siblings — the gap is the installer's `no checksums to verify`
  line). Acceptance: a fresh `curl | sh` transcript shows the inner installer
  verifying the artifact hash. If cargo-dist 0.31 cannot embed for tar.xz,
  record that in V1-CANDIDATES with the upgrade path and keep the wrapper's
  SHA256SUMS verification as the documented integrity story.
- **CHANGELOG 0.1.0 gate**: add the `## 0.1.0` section skeleton (release
  highlights distilled from the rc.2–rc.N CHANGELOG entries) so
  `release-trust.mjs validate` passes for `refs/tags/v0.1.0`; depth-test via
  `node release/trust/release-trust.mjs validate --ref refs/tags/v0.1.0 --repo
  gregce/deadreckon --skip-changelog` vs without.
- **npm wrapper version**: stable requires `npm/deadreckon/package.json` ==
  tag version. Add the bump to the operator checklist AND a test asserting
  the validate gate catches a mismatch (it exists — extend release_plan.rs to
  pin the npm check fires only on the stable lane).

## Real-provider proof harness (P9 spec)

`release/preflight-real.sh` (POSIX sh, operator-run, NEVER in CI):

- For each route in `cli:claude-code cli:codex` (extendable by arg):
  sandboxed `DEADRECKON_HOME`, real `deadreckon start "<tiny goal>" --provider
  <route> --yes` in a throwaway git repo, assert: run completes, gate marker
  validates (`proofs/turn-acceptance.json` + dr-gate signature), `apply`
  succeeds, then a second run is `kill`ed mid-turn and `resume`d to
  completion.
- On success, append/update `release/known-good-providers.json` (schema
  above) using the probed binary version.
- Wire into `docs/RELEASE.md` as a required stable-cut step with expected
  spend note (a few real turns per provider).
- Windows smoke: a checklist subsection (operator): download the signed zip
  on Windows, `signtool verify`, run `deadreckon.exe --version`, `doctor`,
  `try --sandbox none`, record in known-good file under route `windows-smoke`.

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them fail;
implement; green on `cargo fmt --check`, `cargo clippy --workspace
--all-targets` (no NEW errors beyond the pre-existing expect_used debt),
focused `cargo test`; conventional-commit; one-line CHANGELOG entry.

### P1 — Model catalogs + `recommended` (data only)

- Add `recommended: bool` (serde-default) to `ModelEntry`; update
  `tests/.public-surface-baseline` only if the re-export set changes.
- Populate `[[model_catalog]]` for all six CLI descriptors with the ids each
  CLI's `--model` accepts (verify against installed CLIs where present;
  otherwise vendor docs) and for `anthropic`/`openai` http descriptors with
  their current model ids and real per-million costs.
- Exactly one `recommended = true` per descriptor; registry validation
  rejects zero or multiple at parse time (fail-closed like other descriptor
  validation).

Depth tests (in `crates/deadreckon-providers/src/registry/` tests +
`crates/deadreckon/tests/providers_list.rs`):
- `every_builtin_descriptor_has_a_populated_model_catalog`
- `every_builtin_descriptor_has_exactly_one_recommended_model`
- `model_entry_without_recommended_field_still_parses` (old TOML fixture)

### P2 — `deadreckon models` verb

- New subcommand under Inspection; resolves the registry + config, prints a
  `columns` table (`model / context / cost / notes`) per provider with the
  configured default and recommended markers; `--all` includes
  credential-less routes; `--json` additive shape per the signature section.
- Refusal cases per the verb table; lifecycle hint:
  `set it: deadreckon config model <id>`.

Depth tests (`crates/deadreckon/tests/providers_list.rs` or new
`models_cli.rs`):
- `models_lists_catalog_for_explicit_provider_with_recommended_marker`
- `models_json_is_additive_and_names_configured_default`
- `models_unknown_provider_refuses_with_providers_list_try_line`

### P3 — Per-role model flags (orchestrate/campaign)

- Add the six flags per the signature section; reuse the `IDX=VALUE` parser
  from `--child-provider`; thread through `PlanProviders` new fields; child
  spawn argv gains `--model <resolved>` exactly when resolution lands on a
  non-"provider default" id (pin spawn argv like the existing campaign argv
  tests).
- Resolution per the one-rule list; `setup.rs` selection surfaces the model
  per role.

Depth tests (`crates/deadreckon/tests/orchestrate.rs`, `campaign.rs`):
- `full_plan_preview_provider_roles_table_echoes_per_role_models`
- `child_model_idx_flag_overrides_default_child_model_in_spawn_argv`
- `campaign_planner_model_flows_to_sub_orchestrator_argv`
- `pre_rider_plan_json_fixture_deserializes_with_empty_model_fields`

### P4 — Interactive model picker + surface echo

- `start` (after provider resolution, TTY only) and `init` (after provider
  step): a `select_one` over the resolved provider's catalog — label = id,
  detail = `context 200k · recommended` / cost for metered; default cursor on
  the configured default else recommended; choosing "provider default" omits
  the flag. Skipped entirely under `--yes`, `--model`, line constraints
  (same gating list as the rescue spec).
- Audit every preview/verdict surface that prints `provider`: it must print
  the resolved model (or `provider default`); fix the `model=-` gaps found in
  `providers list`-adjacent surfaces.

Depth tests:
- `start_pty_model_picker_appears_after_provider_and_enter_keeps_default`
  (PTY harness, line mode, `\r`)
- `start_with_model_flag_skips_the_picker`
- `run_preview_names_resolved_model_for_cli_provider`

### P5 — Never-dead-end TTY rescue

- Implement the rescue spec verbatim (one rescue, three surfaces, strict
  gating list, hint to persist the choice).
- Extract init's provider menu into a setup-callable
  `provider_rescue_picker(reason: &str)`.

Depth tests:
- `start_pty_with_unusable_default_offers_provider_picker_then_launches`
- `rescue_never_fires_off_tty_refusal_is_byte_identical` (golden vs today)
- `rescue_cancel_choice_produces_existing_refusal_with_try_line`
- `doc_provider_refusal_rescues_on_tty`

### P6 — history.json corrupt-fallback + atomic save

- Per the durability spec: parse-failure → warn → reconstruct from
  traces.jsonl → atomic re-save; `save_history` uses tempfile+rename.

Depth tests (`crates/deadreckon-runtime/src/turn_loop.rs` tests):
- `truncated_history_json_resumes_via_trace_reconstruction`
- `garbage_history_json_resumes_via_trace_reconstruction_and_resaves`
- `history_save_is_atomic_tempfile_rename` (no partial file visible on
  simulated failure)

### P7 — Lock reclaim never usurps an alive pid

- Per the durability spec: alive pid → LockHeld refusal with stale-heartbeat
  note + `try: deadreckon kill <run> --force`; dead pid → reclaim as today.

Depth tests (`crates/deadreckon-core/src/lock.rs` tests):
- `alive_holder_with_ancient_heartbeat_is_not_reclaimed`
- `dead_holder_is_reclaimed_regardless_of_heartbeat_age`
- `lockheld_refusal_names_heartbeat_age_and_force_kill_try_line`

### P8 — Stable-lane gates

- Embedded checksums per spec (or documented V1-CANDIDATES fallback).
- `## 0.1.0` CHANGELOG skeleton; validate-gate depth coverage.
- release_plan.rs: npm-version gate pinned stable-only.

Depth tests (`crates/deadreckon/tests/release_plan.rs`):
- `stable_validate_requires_changelog_section_and_rc_does_not`
- `stable_validate_requires_npm_wrapper_version_match`
- `installer_artifact_checksum_verification_is_documented_or_embedded`
  (asserts whichever path P8 lands)

### P9 — Real-provider proof harness

- `release/preflight-real.sh` + `release/known-good-providers.json` schema +
  RELEASE.md wiring + Windows smoke checklist, per spec. The script must be
  shellcheck-clean POSIX sh and refuse to run when `CI` is set.

Depth tests:
- `preflight_real_script_refuses_under_ci_env` (run the script with CI=1,
  assert refusal text)
- `known_good_providers_schema_round_trips` (serde fixture in a small rust
  test or node --check level validation via release_plan.rs)

### P10 — Operator checklist + docs truth pass

- `docs/RELEASE.md`: new "Stable v0.1.0 operator checklist" section —
  create `gregce/homebrew-tap` repo + `HOMEBREW_TAP_TOKEN`; configure npm
  trusted publishing (or `NPM_TOKEN`) for all six packages; stage
  `WINDOWS_CERT_PFX`/`WINDOWS_CERT_PWD` or consciously narrow the lane (the
  workflow already fails closed); bump `npm/deadreckon/package.json` to
  0.1.0-final; run `release/preflight-real.sh`; Windows smoke; then tag.
- HOWTO.md + help text: `models` verb, the model picker, per-role model
  flags, the rescue behavior. `help-all` catalog entry for `models`.

Depth tests:
- `release_runbook_contains_stable_operator_checklist` (doc-presence pin in
  release_plan.rs, same style as existing runbook pins)
- `help_all_lists_models_verb_in_inspection_group`

### P11 — Architecture doc update + CHANGELOG (doc only; no depth test)

- Insert into `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:
  ```
  ## 43. Stable Readiness (models, rescue, durability, release gates)

  43.1 Model catalogs and resolution order
  43.2 The models verb and picker surfaces
  43.3 Per-role models in plan.json (additive)
  43.4 TTY rescue at refusal sites
  43.5 history.json reconstruction + atomic save; lock reclaim rules
  43.6 Stable-lane gates and the operator checklist
  ```
- Update V1-CANDIDATES: remove/replace entries this rider closes (HTTP retry
  note already closed; add embedded-checksum note only if P8 fell back).
- CHANGELOG section:
  ```
  ## Stable Readiness — 2026-06-XX
  - <one bullet per phase landed>
  ```

## Integration matrix

| surface | model flag | picker | rescue | echoes model |
|---|---|---|---|---|
| run | `--model` (exists) | no (flagged or default) | via start path n/a; run refuses as today off-TTY, rescues on TTY | P4 |
| start | `--model` (exists) | P4 | P5 | P4 |
| chain | `--model` (exists) | no | inherits start/run resolution | P4 |
| orchestrate | P3 per-role | no (flags only) | planner selection rescues on TTY | P3 table |
| campaign | P3 | no | planner rescue | P3 |
| init | n/a (sets default) | P4 | n/a (it IS the picker) | summary line |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| unknown provider for `models` | `deadreckon providers list --all` |
| malformed `--child-model` pair | `deadreckon orchestrate full-plan --child-model 0=sonnet ...` |
| model not in catalog (warning, not refusal) | `deadreckon models <provider>` |
| lock held by alive pid | `deadreckon kill <run-id> --force` |
| rescue declined / cancelled | existing refusal's `try:` line, byte-identical |

## Config additions

```toml
[defaults]
model = "<id>"           # exists via `config model`; no new keys
```

No new config keys. Per-role models are flags + plan.json only (a
`defaults.planner_model` is a V1 candidate, not this rider).

## Out of scope (explicitly not in this milestone)

- Executing any release: tagging v0.1.0, publishing Homebrew/npm, creating
  the tap repo, setting CI secrets (operator checklist items).
- Live model listing from provider APIs (`/v1/models` etc.) — catalogs are
  descriptor-static; dynamic discovery is a V1 candidate.
- `defaults.planner_model`-style per-role config keys.
- Model-based spend estimation changes beyond displaying catalog costs.
- Windows CI test execution (smoke stays an operator checklist).
- Retry-After-aware multi-attempt backoff (separate V1 candidate, already
  logged).

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (utility, free): none expected.
Tier 2 (architectural, log to DEPENDENCIES.md): none expected — inquire is
already landed; P8 uses existing cargo-dist config surface only.
Tier 3 (blocked): unchanged from prior riders (no GPL, no new prompt/styling
stacks beyond the settled inquire decision, no provider SDKs).

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes.** Plan/descriptor additions are
  serde-default additive and fixture-pinned against pre-rider files.
- **One depth test red before each phase implementation.** A phase whose
  tests were never red is suspect.
- **"provider default" means no `--model` argv.** Pin with a spawn-argv test;
  passing a literal `provider default` string to a CLI is a bug.
- **Rescue parity:** off-TTY refusal output is byte-identical to today —
  golden-pinned. The rescue is additive UX, not a behavior change for
  scripts.
- **Catalog warnings never block.** An unknown model passes through with a
  warning; CLIs own their model namespace.
- **No silent expansion.** Anything beyond P1–P11 goes to V1-CANDIDATES.md.
- **PTY tests pin line mode** (`DEADRECKON_PROMPT_LINE_MODE=1`) and send
  `\r`-terminated answers; inquire is exercised by its own unit seams, not by
  driving raw digits at a Select.

## Process invariants

- Phased local commits only. No `git push`, no tags.
- Each phase ends with the relevant depth tests passing and a CHANGELOG entry.
- `cargo test --workspace --locked --no-fail-fast` green before declaring the
  rider done (54+ binaries; the count grows with new test files).
- If a phase reveals a V1-architecture decision, stop and log it in
  V1-CANDIDATES.md; do not silently expand scope.
- After P11, optionally capture a short asciinema demo of `models` + the
  picker + a rescue under `/Users/gdc/deadreckon/` (user-visible slice —
  worth it).
