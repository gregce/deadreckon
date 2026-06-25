# deadreckon — Rules-as-gate Rider (the operator's conventions, enforced)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-06-25-1423-deadreckon-rules-gate-goal.md`.
It supersedes nothing in prior riders (tamper-evident, composable-seams,
stable-readiness); their invariants still apply. This rider adds one
`AcceptanceCheck` variant (`Rules`), a `deadreckon-rules.yaml` format,
deterministic touched-file evaluation, and tamper binding of the rules file.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime `~/.deadreckon`.

## Posture (decided — do not redesign)

- **Maturity stays stable** (0.3.1 shipped; lands under a `Rules` CHANGELOG section).
- **One additive `AcceptanceCheck::Rules` variant only.** This is the sanctioned gate extension seam (new check kinds = new enum variant + parser + `evaluate_check` arm + tamper `check_coverage` arm). Serde-additive and backward-compatible: old specs without it still parse. **No `PipelineState`/`AcceptanceMarker` schema changes.**
- **Rule evaluation is deterministic.** Regex + glob over file bytes. No provider call, no LLM — the gate is the trust boundary; what counts as a violation must be reproducible.
- **The gate trust contract is reused, not modified.** Rules ride the existing `evaluate_acceptance` → sign path; nonce isolation, `validate_acceptance_marker`, and the dr-gate subprocess boundary are untouched, except `marker_signature` additionally folds in the rules-file bytes (the existing optional-sidecar pattern at gate.rs:777-801).
- **Judged on what changed.** Rules evaluate against the run's touched files (provenance) by default; whole-tree scanning is per-rule opt-in (`scope: all`).
- **No `git push`.** Phased local commits only.
- **No V1 invention.** AST-aware rules, severity tiers/warn-only, autofix, shared rule packs, and language-server integration go to V1-CANDIDATES.
- **Edits stay inside `/Users/gdc/deadreckon`.**

## Data model (files, not fields)

`deadreckon-rules.yaml` (in the working dir; path overridable in the check):

```yaml
rules:
  - id: no-unwrap-in-src        # required, unique, kebab-case; used in refusals
    forbid: '\.unwrap\(\)'      # regex; exactly one of forbid|require
    paths: ['src/**/*.rs']      # globs the rule applies to
    exclude: ['**/tests/**']    # optional globs subtracted from paths
    message: 'use ? or .expect("ctx"), not unwrap()'
    scope: touched              # touched (default) | all
  - id: require-license-header
    require: 'SPDX-License-Identifier'
    paths: ['src/**/*.rs']
    message: 'every source file needs an SPDX header'
```

Schema: `RuleSet { rules: Vec<Rule> }`, `Rule { id, forbid?: String, require?: String, paths: Vec<String>, exclude: Vec<String> (default []), message: String, scope: Scope (default Touched) }`, `enum Scope { Touched, All }`. Exactly one of `forbid`/`require` per rule (a parse error otherwise). Patterns compile once; an invalid regex is a parse-time refusal, not a silent skip.

New `AcceptanceCheck` variant:

```
AcceptanceCheck::Rules {
    #[serde(default = "default_rules_path")]  // "deadreckon-rules.yaml"
    path: String,
    #[serde(default = "default_must_pass")]
    must_pass: bool,
}
```

## Evaluation rules (the spec — match it in code)

`crates/deadreckon-core/src/rules.rs`:

```
fn load_ruleset(path: &Path) -> Result<RuleSet>
fn evaluate_rules(ruleset: &RuleSet, working_dir: &Path, touched: &[PathBuf]) -> Vec<RuleViolation>
struct RuleViolation { rule_id: String, file: PathBuf, line: usize, message: String, kind: ForbidOrRequire }
```

- For each rule, resolve its file set: `scope: touched` → intersection of `touched` with `paths` minus `exclude`; `scope: all` → glob the tree under working_dir with the same paths/exclude.
- `forbid`: a violation per matching (file, line). `require`: a violation per file in the set that contains NO match (reported at line 0/file-level).
- Result is folded into a single `AcceptanceCheckResult { kind: "rules", passed: violations.is_empty() || !must_pass, detail: "<n> rule violation(s): …", … }`, with the first K violations rendered as `<rule-id> <file>:<line> — <message>` in `detail`/`stderr` for the corrective hint.
- Deterministic ordering: rules in file order, then files sorted, then line ascending — so the same run produces the same detail bytes (the detail is part of what gets signed via the check result).

## Wiring

- `parse_acceptance_checks` (gate.rs:614+): add the `Rules` arm (serde already covers it; ensure the YAML kind tag `rules` maps).
- `evaluate_check` (gate.rs:437): add the `Rules { path, must_pass }` arm → `load_ruleset` (relative to working_dir) → `evaluate_rules` with the run's touched files → `AcceptanceCheckResult`. A missing rules file when a `Rules` check is explicitly listed is a refusal (`must_pass` check fails with "rules file not found"); auto-inclusion (below) only adds the check when the file exists.
- `compiled_acceptance_checks` (gate.rs:284): when no operator spec exists AND `deadreckon-rules.yaml` is present, append a `Rules { path: default, must_pass: true }` to whatever language defaults were compiled (composes with the Polyglot floor if present; independent if not).
- Touched files come from the same provenance/snapshot source `tamper.rs::touched_files` uses — rules and tamper agree on "what changed".

## Tamper binding (the anti-gutting protection)

In `crates/deadreckon-core/src/{gate.rs,tamper.rs}`:

- `marker_signature` (gate.rs:762-803): fold the bytes of every `Rules` check's resolved rules file into the signature (reuse the optional-sidecar read-and-hash block). Editing the rules file after signing invalidates the marker.
- `tamper.rs`: extend the protected-spec logic that already refuses when the agent edits `acceptance.yaml` so it ALSO refuses when the agent edits or deletes a `deadreckon-rules.yaml` the run was subject to, or removes a rule `id` that was present at the earliest snapshot (weakening). `check_coverage` gains a `Rules` arm classifying the rules file + its covered paths.
- Write the gutting depth tests FIRST: delete the rules file → refuse; remove a rule id mid-run → refuse; loosen a `forbid` pattern mid-run → refuse.

## Phases (eleven)

Each phase: named depth test(s) **first** (watch fail) → implement → `make verify` green (fmt-check, clippy, public-surface, test, build) → conventional-commit → one-line CHANGELOG entry naming the SHA.

### P1 — Rules schema + parser module
- `rules.rs`: `RuleSet`/`Rule`/`Scope`, `load_ruleset`, exactly-one-of forbid|require validation, invalid-regex-is-parse-error.

Depth tests (`crates/deadreckon-core/src/rules.rs`):
- `parse_minimal_forbid_rule`
- `rule_with_both_forbid_and_require_is_parse_error`
- `invalid_regex_is_parse_error`

### P2 — `AcceptanceCheck::Rules` variant + serde + parse arm
- Add the variant + defaults; extend `parse_acceptance_checks`; confirm old specs still parse (no variant present).

Depth tests (gate tests):
- `acceptance_yaml_with_rules_check_parses`
- `legacy_acceptance_yaml_without_rules_still_parses`

### P3 — Forbid/require evaluation against a file set
- `evaluate_rules` for an explicit file list: forbid → per-match violations with line numbers; require → file-level violation when absent.

Depth tests:
- `forbid_pattern_reports_violation_with_line`
- `require_pattern_absent_reports_file_level_violation`
- `rule_respects_paths_and_exclude_globs`

### P4 — Touched-file scoping + `scope: all`
- Default `Touched` intersects with provenance touched files; `All` globs the tree. Wire the run's touched set.

Depth tests:
- `touched_scope_ignores_unchanged_files`
- `all_scope_scans_whole_tree`

### P5 — `evaluate_check` Rules arm → AcceptanceCheckResult
- Fold violations into one deterministic `AcceptanceCheckResult`; render first-K violations into detail/stderr; stable ordering.

Depth tests:
- `rules_check_passes_when_no_violations`
- `rules_check_fails_and_lists_violations_deterministically`
- `missing_rules_file_for_explicit_check_refuses`

### P6 — Auto-include when `deadreckon-rules.yaml` present
- `compiled_acceptance_checks` appends a `Rules` check when the file exists and no operator spec overrides; composes with language defaults.

Depth tests:
- `rules_file_presence_auto_adds_rules_check`
- `auto_rules_check_composes_with_language_default`
- `operator_spec_still_overrides_auto_rules`

### P7 — Tamper: rules bytes in marker signature
- Fold rules-file bytes into `marker_signature`; editing the file after signing invalidates the marker.

Depth tests:
- `editing_rules_file_after_sign_invalidates_marker`
- `marker_signature_includes_rules_bytes`

### P8 — Tamper: gutting refuses
- Refuse on delete/edit of a subjected rules file and on rule-id removal/weakening mid-run; `check_coverage` Rules arm.

Depth tests:
- `deleting_rules_file_mid_run_refuses`
- `removing_a_rule_id_mid_run_refuses`
- `loosening_forbid_pattern_mid_run_refuses`

### P9 — Non-terminal gate feedback
- A failed rules check feeds the rule id + file:line back into the agent's history as a corrective hint (reuse the acceptance-failed hint path), letting the agent fix and retry rather than dying.

Depth tests:
- `rules_failure_pushes_corrective_hint_with_rule_id`
- `rules_failure_is_non_terminal_until_max_turns`

### P10 — Friendliness: preview + detect + refuse-with-try + verdict
- Preview active rules in run preflight and `detect`; refuse with `try:` naming the violated rule + file:line; surface rule violations in `verdict` evidence (if Verdict has landed; otherwise behind the shared check-result rendering). Honor `--quiet`/`--plain`/`--json`.

Depth tests:
- `detect_lists_active_rules`
- `rule_violation_refuses_with_try_naming_rule_and_location`
- `preflight_preview_lists_active_rules`

### P11 — Architecture doc update + CHANGELOG (doc only; no depth test)
- Insert into `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:
  ```
  ### 13.x Rules-as-gate (deadreckon-rules.yaml)
  ### 35.x Rules tamper binding
  ```
  documenting the rules schema, touched-file scoping, the signature binding, and the gutting-refuses protection.
- Update §22 "What's Built vs Scaffolding-Thin": add Rules-as-gate to shipped; note it answers "agents ignore my rules" by making conventions an enforced, tamper-evident gate check, adding exactly one `AcceptanceCheck` variant.
- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:
  ```
  ## Rules (stable) — 2026-06-25
  - deadreckon-rules.yaml lets operators declare forbidden/required patterns the gate enforces against touched files; rules are tamper-bound into the marker signature so an agent cannot pass by deleting or weakening a rule.
  ```

## Integration matrix

| Concern | Behavior |
|---|---|
| No rules file | no Rules check; gate unchanged |
| Rules file present, no violations | Rules check passes; marker signs over rules bytes |
| Forbidden pattern in touched file | gate refuses non-terminally; corrective hint with rule id + file:line |
| Rules file edited/deleted mid-run | tamper `refuse` |
| Operator `acceptance.yaml` lists explicit Rules check | honored; missing file → refuse |
| Composes with Polyglot language default | appended alongside the language test check |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| rule violated | `try: fix <rule-id> at <file>:<line> — <message>` |
| rules file referenced but missing | `try: add ./deadreckon-rules.yaml or remove the rules check` |
| invalid regex in rules file | `try: fix the pattern for <rule-id> in deadreckon-rules.yaml` |
| both forbid and require on one rule | `try: split <rule-id> into two rules` |

(Each parameterized by a depth test.)

## Out of scope (explicitly → V1-CANDIDATES)

- AST/semantic rules (only regex+glob here).
- Severity tiers / warn-only rules (every rule is must-pass in this slice).
- Autofix of violations.
- Shared/importable rule packs and remote rule sources.
- Per-rule provider/LLM judgement (rules are deterministic by design).
- Language-server / editor integration.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (in tree, free): `regex` (already a workspace dep), `ignore`/glob support (already present), `serde`/YAML path (reuse acceptance parsing), `std::fs`. Tier 2 (architectural → DEPENDENCIES.md): none expected. Tier 3 (blocked): no network, no new heavy crates.

## Engineering invariants (do not violate)

- **Exactly one new `AcceptanceCheck` variant** (`Rules`); no `PipelineState`/`AcceptanceMarker` schema changes.
- **Rule evaluation is deterministic and LLM-free** — reproducible violations, stable ordering, no provider in the path.
- **Rules are tamper-bound** — file bytes in the signature, gutting refuses, uniformly with the acceptance.yaml protection.
- **Judged on touched files by default** — whole-tree scanning is explicit opt-in.
- **One depth test before each phase.** A phase whose tests were never red is suspect.
- **Non-terminal failure** — a violated rule feeds back a hint and lets the agent retry; it does not crash the run.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth tests passing and a CHANGELOG entry naming the SHA.
- Tests build fixture working dirs (tempfile) with planted violations + a rules file; tamper tests fabricate snapshots to simulate mid-run edits. No live provider calls.
- If a phase reveals a V1-architecture decision, stop and log it in V1-CANDIDATES; do not silently expand scope.
