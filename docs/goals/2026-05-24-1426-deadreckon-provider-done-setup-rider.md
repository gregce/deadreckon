# deadreckon - Provider And Done-Criteria Setup Rider (Prepared)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-24-1426-deadreckon-provider-done-setup-goal.md`.
It supersedes nothing in prior riders
(`2026-05-17-1403-deadreckon-coherence-closure-rider.md`,
`2026-05-11-2122-deadreckon-doc-depth-rider.md`,
`2026-05-11-2248-deadreckon-provider-registry-rider.md`,
`2026-05-18-2226-deadreckon-orchestration-eventbus-rider.md`,
`2026-05-18-2336-deadreckon-implementation-notes-rider.md`) - their
invariants still apply. This rider turns the P2/P5 V1 deferrals into an
executable implementation pass: one setup substrate for provider choices and
one setup substrate for done criteria.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided - do not redesign)

- **Maturity stays `alpha`.**
- **No `PipelineState` schema changes.** Run/plan state must keep loading.
- **No new top-level verb.** `def-done` remains canonical; hidden
  `acceptance` remains compatibility.
- **Provider descriptors remain provider descriptors.** Do not grow descriptor
  schema for setup wording unless an existing field cannot express install or
  detection state.
- **Avoid new durable config keys.** Existing keys include
  `default_provider`, `fallback`, `[defaults].provider`,
  `[defaults].doc_provider`, model config, and provider tables. If a phase proves
  a tiny additive key is needed, document it in AS-BUILT and keep old config
  valid.
- **User-facing vocabulary:** use "provider", "model", "provider role", and
  "done criteria". Use `acceptance.yaml` only for file paths and technical rows;
  use "gate" only for signed-check execution details.
- **No `git push`.** Phased local commits only.
- **No `make verify`, release builds, stress tests, or full-workspace tests by
  default.** Use focused verification unless the implementation expands the
  touched surface.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Current assessment

The CLI already has the pieces:

- `init_command` prompts or auto-selects a provider, then writes
  `config.toml`.
- `auto_subscription_cli_provider` finds installed subscription CLI providers.
- `config provider` writes `defaults.provider` and `default_provider`.
- `print_provider_selection` renders current provider routes.
- `resolve_doc_provider` separately resolves doc polish provider from flag,
  config, auto subscription, run provider, or none.
- `run`, `extend`, `resume`, `plan`, `fork`, `merge`, and `orchestrate` already
  carry provider-role concepts.
- `ensure_acceptance_before_start`, `resolve_acceptance_source`,
  `acceptance_preview`, `copy_acceptance_into_run`,
  `acceptance_explain_command`, and hidden `acceptance` commands already manage
  `.deadreckon/acceptance.yaml` and `.deadreckon/acceptance.md`.

The problem is not missing capability. The problem is local setup logic:
provider/default/doc-provider/orchestration role paths do not share one
resolver, and done-criteria wording/help/preflight is split between `def-done`,
hidden `acceptance`, run previews, orchestration previews, status/gate rows, and
doc polish.

## Data model (runtime structs, not durable fields)

Names may adjust to local Rust style. Prefer a small module such as
`crates/deadreckon/src/setup.rs` if it keeps `main.rs` from growing. Keeping
the code in `main.rs` is acceptable only if the extracted APIs are still clear
and tested.

```rust
pub enum SetupProviderRole {
    PrimaryRun,
    ConfigDefault,
    DocPolish,
    Planner,
    DefaultChild,
    ChildOverride { index: usize },
    Coder,
    Reviewer,
    Repair,
}

pub enum SetupProviderSource {
    Flag,
    Config,
    AutoSubscription,
    RunProvider,
    BuiltInDefault,
    None,
}

pub struct ProviderSetupRequest<'a> {
    pub role: SetupProviderRole,
    pub explicit_provider: Option<&'a str>,
    pub explicit_model: Option<&'a str>,
    pub config_default_provider: Option<&'a str>,
    pub config_doc_provider: Option<&'a str>,
    pub run_provider: Option<&'a str>,
    pub allow_auto_subscription: bool,
    pub require_usable_route: bool,
}

pub struct ProviderSetupSelection {
    pub role: SetupProviderRole,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub source: SetupProviderSource,
    pub kind: Option<String>,
    pub credential: Option<String>, // ready | missing | subscription | none
    pub install_hint: Option<String>,
    pub warnings: Vec<String>,
    pub try_lines: Vec<String>,
}
```

Done-criteria setup:

```rust
pub enum DoneCriteriaSource {
    ExplicitPath,
    ProjectFile,
    Generated,
    DefaultGate,
}

pub struct DoneCriteriaSelection {
    pub source: DoneCriteriaSource,
    pub path: Option<PathBuf>,
    pub companion_doc: Option<PathBuf>,
    pub checks: Option<usize>,
    pub label: String,      // e.g. "project (3 checks)"
    pub technical_label: String, // e.g. "acceptance.yaml"
    pub try_lines: Vec<String>,
}
```

Do not serialize these structs unless a later phase proves a durable need.

## Provider setup rules

Provider resolution must be one function family, not separate hand-coded logic
per command.

Base priority:

1. Explicit flag/argument for the role, if present.
2. Role-specific existing config when present (`defaults.doc_provider` for doc
   polish; existing plan/provider flags for orchestration role requests).
3. Existing default provider config (`defaults.provider`, then
   `default_provider`).
4. Auto subscription CLI provider when the request allows it and a subscription
   CLI binary is available through descriptor detection.
5. Run provider only for doc polish/follow-up contexts where that is the current
   behavior.
6. Built-in fallback only where the command currently has one and it remains
   intentional.
7. None/refusal with concrete `try:` lines.

Validation:

- Unknown provider route refuses before writing config or starting work.
- Missing credentials for API providers are surfaced as setup warnings or
  refusal depending on whether the command is only previewing/listing.
- Subscription CLI routes should display as subscription/ready when the binary
  is available.
- `config provider <id>` should use the same validation and install hints as
  `init` and run/orchestrate preflight.
- `doc --polish` should keep the doc-depth priority behavior but use the shared
  source labels (`flag`, `config`, `auto_subscription`, `run_provider`, `none`).

Output contract:

```text
provider setup:
  primary:  cli:codex        model=provider default source=config credential=subscription
  docs:     cli:codex        model=provider default source=config credential=subscription
  planner:  cli:codex        model=provider default source=flag   credential=subscription
try: deadreckon config provider cli:codex
```

The exact spacing may follow existing `print_kv_block`/orchestration table
style, but labels and source words should be shared.

## Done-criteria setup rules

Done-criteria selection must be one function family used by run, orchestrate,
plan/fork child launches, status/preview text, and `def-done` inspection.

Priority:

1. Explicit `--acceptance <path>` resolves relative to cwd, refuses if missing,
   and labels source as `explicit`.
2. Project `.deadreckon/acceptance.yaml` resolves with optional companion
   `.deadreckon/acceptance.md`, labels source as `project`, and reports check
   count.
3. Interactive generation through `def-done`/setup prompt may produce criteria
   and labels source as `generated`.
4. Default dr-gate behavior labels source as `default`, with clear explanation.

Shared labels:

- Human-facing: "done criteria".
- File-facing: `.deadreckon/acceptance.yaml`, `.deadreckon/acceptance.md`.
- Execution-facing: "gate" only when describing `dr-gate` or marker validation.

Non-interactive behavior:

- Never prompt when stdin is not a terminal.
- If criteria are required by a preview or start path and cannot be generated,
  refuse with `try: deadreckon def-done "what should count as done"`.
- Existing default-gate behavior must remain available where it is current
  behavior; this goal standardizes the display, not by itself making custom
  criteria mandatory everywhere.

## Command surface impact

No new top-level verbs.

Affected commands and expected use of shared setup:

```text
deadreckon init [--provider <id>] [--api-key <key>] [--no-confirm]
deadreckon config provider [<id>]
deadreckon config model [<model>] [--provider <id>]
deadreckon run <goal> [--provider <id>] [--model <model>] [--doc-provider <id>] [--acceptance <path>] [--preview]
deadreckon orchestrate ... [--acceptance <path>] [--yes|--preview]
deadreckon plan ... [--acceptance <path>] [provider role flags...]
deadreckon doc <run-id> --polish [--doc-provider <id>] [--no-confirm]
deadreckon def-done [add|edit|show|check] ...
deadreckon acceptance ... # hidden compatibility; shares text/resolution
```

Do not rename flags in this goal. Hidden alias removal remains a separate V1
decision.

## Refusal cases

| Case | Message shape | Try line |
|---|---|---|
| Unknown provider route | `unknown provider route <id>` | `deadreckon providers list --all` |
| API provider missing key | `provider <id> needs credentials` | `deadreckon config set providers.<id>.api_key <KEY>` or env hint |
| CLI provider binary missing | `provider <id> binary not found` | descriptor install hint |
| No provider for doc polish | `no doc provider available` | `deadreckon config set defaults.doc_provider cli:codex` |
| Missing explicit acceptance path | `done criteria file not found: <path>` | `deadreckon def-done "what should count as done"` |
| Non-interactive prompt would be required | `non-interactive setup requires <flag>` | command-specific rerun with `--yes`, `--no-confirm`, or `def-done` |
| Invalid acceptance YAML | `invalid done criteria` | `deadreckon def-done edit "fix the criteria"` |

## Verification budget

Use focused checks by phase. Prefer:

```sh
cargo test -p deadreckon coherence
cargo test -p deadreckon self_documenting
cargo test -p deadreckon def_done
cargo test -p deadreckon acceptance
cargo test -p deadreckon --test orchestrate provider
cargo test -p deadreckon --test orchestrate acceptance
cargo test -p deadreckon doc_depth
cargo test -p deadreckon providers_list
cargo test -p deadreckon detect
cargo fmt --check
cargo clippy -p deadreckon --tests -- -D warnings
```

If tests are added to a new module, add the smallest matching filter. Do not run
`make verify`, release builds, smoke, stress, or full-workspace tests by
default.

## Phases (eleven)

Each phase: write the named depth tests first and watch them fail; implement;
run focused verification for the touched surface; local conventional commit;
one-line CHANGELOG entry.

### P1 - Inventory and public contract

- Add a concise source comment or doc section near the new setup module naming
  provider setup and done-criteria setup as shared command substrates.
- Confirm all current setup call sites and decide whether to create
  `setup.rs` or a clearly bounded section in `main.rs`.
- Do not change behavior yet except tests that pin the desired contract.

Depth tests:

- `setup_contract_lists_provider_and_done_call_sites`
- `setup_source_labels_are_shared_across_provider_roles`
- `done_criteria_labels_reserve_acceptance_for_file_paths`

### P2 - Provider setup resolver

- Implement runtime provider setup request/selection types.
- Use `ProviderRegistry::with_overrides`, `ProviderRouter`, route info, and
  descriptor install hints where possible.
- Preserve existing config/default fallback semantics.
- Preserve doc polish's flag -> config -> auto subscription -> run provider ->
  none semantics.

Depth tests:

- `provider_setup_prefers_flag_over_config`
- `provider_setup_uses_config_default_for_primary_run`
- `provider_setup_auto_subscription_uses_installed_cli_descriptor`
- `provider_setup_doc_polish_preserves_source_order`
- `provider_setup_unknown_route_refuses_with_providers_list_try_line`

### P3 - `init` and `config provider`

- Replace `prompt_provider` and direct `config provider` writes with the shared
  resolver/validator.
- `init --no-confirm` should choose the same auto subscription provider as the
  resolver.
- `config provider` with no argument should render the same setup vocabulary as
  previews.
- `config provider <id>` should refuse unknown routes before writing config.

Depth tests:

- `init_no_confirm_and_config_provider_use_same_auto_provider`
- `config_provider_rejects_unknown_route_without_writing_config`
- `config_provider_prints_model_source_and_credential`

### P4 - Run provider/doc-provider preflight

- Fresh, extended, and resumed run startup summaries should consume provider
  setup selections for primary and docs rows.
- Keep existing run behavior and `print_run_started` shape where reasonable,
  but stop recomputing provider/doc-provider source labels locally.
- `--provider`, `--model`, and `--doc-provider` should share validation and
  source labels.

Depth tests:

- `run_preview_primary_provider_uses_setup_selection`
- `run_preview_doc_provider_source_matches_doc_polish_source`
- `extend_and_resume_preserve_shared_provider_setup_rows`

### P5 - Doc polish provider setup

- Route `doc --polish` through the shared provider setup resolver.
- Keep current preview fields: provider, source, subskills, token budget,
  estimate, cost cap, inputs.
- Replace bespoke no-provider wording with the shared refusal table.

Depth tests:

- `doc_polish_uses_shared_doc_provider_selection`
- `doc_polish_no_provider_refusal_has_setup_try_line`
- `doc_polish_preview_provider_source_matches_run_preview`

### P6 - Done-criteria resolver

- Extract shared done-criteria selection from `resolve_acceptance_source`,
  `acceptance_preview`, `resolve_acceptance_path_for_command`, and
  `ensure_acceptance_before_start` without breaking their behavior.
- Preserve copying of `acceptance.yaml`, `acceptance.md`, and helper files into
  run roots.
- Preserve default dr-gate behavior.

Depth tests:

- `done_setup_prefers_explicit_acceptance_path`
- `done_setup_uses_project_acceptance_yaml_and_md`
- `done_setup_default_gate_label_is_shared`
- `done_setup_missing_explicit_path_refuses_with_def_done_try_line`

### P7 - Run/orchestrate done-criteria preflight

- Make `run --preview`, run start summaries, `orchestrate --preview`, `plan`,
  and child launch setup use the shared done-criteria selection.
- For the same workspace and explicit flags, run and orchestrate should show the
  same source/check-count label.
- Keep generated interactive criteria behavior, but drive it through the shared
  setup wording.

Depth tests:

- `run_and_orchestrate_preview_show_same_done_criteria_source`
- `orchestrate_child_runs_copy_shared_done_criteria_selection`
- `noninteractive_done_setup_refuses_instead_of_prompting`
- `generated_done_criteria_reports_generated_source`

### P8 - `def-done` and hidden `acceptance` docs parity

- Make `def-done help`, `def-done show`, hidden `acceptance explain`, and
  command help share one text source for done-criteria explanation.
- Keep `acceptance` hidden and compatibility-focused.
- User-facing rows should say "done criteria"; file paths may still say
  `.deadreckon/acceptance.yaml`.

Depth tests:

- `def_done_and_acceptance_explain_share_done_criteria_header`
- `def_done_help_and_acceptance_help_do_not_diverge_on_primary_examples`
- `hidden_acceptance_keeps_compatibility_but_points_to_def_done`

### P9 - Setup preview/render helpers

- Add small render helpers for provider setup rows and done-criteria rows.
- Reuse existing `ui` style facade, status tones, command/hint helpers, and
  orchestration role vocabulary.
- Do not build the full output-layout facade in this goal.

Depth tests:

- `provider_setup_rows_are_ansi_free_when_plain`
- `setup_try_lines_use_shared_command_style`
- `done_criteria_rows_fit_run_and_orchestrate_previews`

### P10 - Regression and compatibility sweep

- Audit public docs/help text touched by setup.
- Confirm no `PipelineState` field changes.
- Confirm config files without new keys still load.
- Confirm hidden aliases and existing flags still parse.
- Refresh or add focused goldens only where stable enough.

Depth tests:

- `legacy_config_without_doc_provider_still_loads`
- `hidden_acceptance_command_still_parses`
- `public_docs_do_not_teach_acceptance_as_primary_user_word`
- `pipeline_state_schema_unchanged_for_setup_goal`

### P11 - Architecture docs and changelog

- Update `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:
  - Provider setup/resolution in provider/config sections.
  - Done-criteria setup in acceptance gate and user-facing sections.
  - Built-vs-thin accounting: provider/done setup unification shipped.
- Update `/Users/gdc/deadreckon/docs/design/USER-FACING-MATRIX.md`:
  - Mark P2 and P5 fixed or partly fixed with evidence.
  - Update L9/L10/O2 notes if the implementation closes them.
- Update `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`:
  - Remove or narrow "Provider and done-criteria setup unification".
  - Leave broader output-layout facade, command-matrix goldens, and alias
    removal as V1 items.
- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:

```markdown
## Provider and done-criteria setup unification (alpha) - 2026-05-24

- Unified provider selection and done-criteria setup across init, config,
  run, orchestration, doc polish, and def-done surfaces.
```

No depth test for P11 beyond docs/coherence checks.

## Out of scope

- Full output-layout facade.
- Command-family alias removal.
- New provider descriptor schema for ranking or benchmarks.
- Automatic provider selection by quality/cost/latency.
- New durable plan/run schema fields.
- Making custom done criteria mandatory for every run.
- Web/cloud setup wizard.

## Stop conditions

Stop only when:

- Shared provider setup and done-criteria setup are used by the named command
  surfaces.
- Focused verification passes.
- AS-BUILT, USER-FACING-MATRIX, V1-CANDIDATES, and CHANGELOG are updated.
- No provider routing, doc polish, hidden `acceptance`, signed gate, or
  orchestration role behavior regresses.
- Work is committed locally with one or more conventional commits.
