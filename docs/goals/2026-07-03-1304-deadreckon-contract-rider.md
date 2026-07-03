# deadreckon — Contract Rider (a definition of done you can trust before you spend)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-07-03-1304-deadreckon-contract-goal.md`.
It supersedes nothing in prior riders (polyglot-gate, verdict, rules-gate,
course, helm) — their invariants still apply. This rider adds: a **compiled
contract read model**, a **goal-aware + execution-oriented compiler prompt**,
a deterministic **falsifiability lint**, one clamped **critic pass**, a
**goal↔contract reconciliation** at start, and a real **accept / re-prompt /
edit review loop** surfaced at authoring and on the Course card.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime `~/.deadreckon`.

Framing creed: the done contract is the only thing standing between a launch
and a false `VERIFIED`. It must be derived from the same intent the run
executes against (the goal), it must test what the software *does* (not what
its source *says*), and a human must be able to see it and push back before a
dollar is spent. Everything here serves those three.

## Posture (decided — do not redesign)

- **Maturity stays stable** (0.5.0 shipped; lands under a `Contract` CHANGELOG section).
- **No `PipelineState` or acceptance-file schema changes.** The durable contract stays exactly what it is today: `.deadreckon/acceptance.yaml` + `.deadreckon/acceptance.md` + helper scripts under `.deadreckon/acceptance/`. The compiled contract is a **read model** (a projection over those files), never a new persisted schema.
- **The four check kinds are frozen** — `file_exists`, `content_match`, `shell`, `cargo_test`. This slice forces good behavior through the **compiler prompt** and the **lint**, not by inventing check kinds. A behavioral check is a `shell` check that builds/starts/drives/asserts, or a `cargo_test`/test-runner invocation with real oracles.
- **Deterministic-first.** The falsifiability lint runs with zero provider calls and is the floor. The critic is exactly **one** bounded provider call plus at most **one** automatic re-draft; it never blocks the human — whatever survives is shown for accept/re-prompt/edit.
- **The human is never bypassed silently.** Non-interactive/`--yes` may skip the review loop, but goal↔contract divergence still surfaces, and a strong divergence can refuse with `try:`.
- **No new state-changing surface inside the review** — re-prompt routes through the existing `acceptance` Refine path; edit opens the existing files; check dry-runs the existing gate.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Anything past P1–P11 → `docs/V1-CANDIDATES.md`.
- **Edits stay inside `/Users/gdc/deadreckon`.**

## Data model (read models — files in, projections out)

### CompiledContract (the projection the human and the card read)

```
enum CheckKind { FileExists, ContentMatch, Shell, CargoTest }

struct CompiledCheck {
    index: u32,
    kind: CheckKind,
    summary: String,        // one plain line: "builds with npm run build" |
                            // "scans source for financial vocabulary" |
                            // "runs node .deadreckon/acceptance/check_*.js"
    behavioral: bool,       // executes the product (build/start/run/test)
                            // vs. inspects source text or file presence
    can_fail: bool,         // a plausible wrong implementation would fail it
                            // (false for keyword greps and --if-present-only)
    raw: serde_json::Value, // the original yaml node, verbatim
}

struct CompiledContract {
    name: String,
    md_criteria: String,    // the user-facing restatement from acceptance.md
    checks: Vec<CompiledCheck>,
    source_path: PathBuf,   // .deadreckon/acceptance.yaml
}

fn compile_contract(yaml: &str, md: Option<&str>) -> Result<CompiledContract> // pure
```

`behavioral` / `can_fail` are classified deterministically from the raw node
(see the lint rules). `summary` is a fixed, testable phrasing per kind — the
card and the review both render it, so its wording is depth-tested.

### ContractDivergence (goal↔contract reconciliation)

```
struct ContractDivergence {
    goal_clauses: Vec<String>,     // atomic requirements lifted from the goal
    uncovered: Vec<String>,        // clauses no check plausibly covers
    weak: Vec<LintFinding>,        // falsifiability findings (below)
}

fn reconcile(goal: &str, c: &CompiledContract) -> ContractDivergence  // deterministic
```

`goal_clauses` is a deterministic split of the goal into requirement phrases
(sentence/conjunction/imperative-verb segmentation — no provider). `uncovered`
is a clause whose salient nouns/verbs appear in no check `raw`/`summary`. This
is intentionally a keyword-coverage heuristic: it is a **hint that surfaces
drift**, not a proof; the critic (P5) does the semantic version.

### LintFinding (the falsifiability lint — deterministic floor)

```
enum LintFinding {
    NoBehavioralCheck,                 // no check has behavioral == true
    OnlySourceScanIsSubstantive { index },  // the only non-trivial gate greps source
    IfPresentOnlyBuildOrTest { index },     // build/test uses --if-present with no script present
    UnfalsifiableCheck { index },           // can_fail == false on a substantive check
}
```

## Compiler prompt spec (P3 — the heart of the slice)

`acceptance_agent_prompt` is rewritten. It MUST, verbatim and depth-tested for
presence, instruct the model to:

1. **Derive the contract from the run goal**, not only the acceptance request. The goal is provided (see P2); the request refines it.
2. **Prefer checks that execute the software and observe outputs** — build, start the app, drive it (headless browser / HTTP / CLI invocation), and assert on the result; unit/integration tests with **known inputs → known expected outputs**.
3. Treat **source-text scanning (keyword/vocabulary greps) as INSUFFICIENT** as the sole substantive check. A helper script is allowed only when it *runs* the product or asserts computed results, not when it greps for the presence of words.
4. Make **every substantive check falsifiable**: there must be a plausible wrong implementation that fails it. State this as the bar the model must meet.
5. **Never rely on `--if-present` as the only build/test gate** — if the project lacks a build/test script, author a minimal real one or a direct invocation instead.
6. Keep the four kinds; put any helper under `.deadreckon/acceptance/` and call it from a `shell` check; restate the criteria in `acceptance.md` before listing checks.

The existing anti-self-attestation clause stays. The JSON envelope
(`acceptance_yaml` / `acceptance_md` / `files`) is unchanged.

## Critic pass spec (P5 — one clamped call)

```
struct CriticVerdict {
    stub_would_pass: bool,               // a keyword-only stub satisfies the contract
    uncovered_goal_clauses: Vec<String>,
    weak_check_indices: Vec<u32>,
    verdict: Pass | Redraft,
}
```

After a draft compiles and passes YAML validation, run the critic prompt once
against `{goal, compiled_contract, lint_findings}`. If `Redraft`, fold the
critique into the request and re-draft **exactly once** via the Draft path,
then stop and present whatever resulted (Pass or not) to the human. The critic
is skipped entirely when no provider is configured (lint-only floor still
applies). One critic + one redraft is the hard ceiling — no loops.

## Verb signatures

```
deadreckon acceptance draft <request>
    [--goal <text>]      # NEW: run goal threaded into the compiler; start passes it automatically
    [--provider <p>] [--model <m>] [--force]
    # after compile: renders CompiledContract + divergence, then the review loop

deadreckon acceptance refine <request>
    # unchanged entry; now the re-prompt target — receives goal + prior draft + note

deadreckon start <goal> ...
    [--review-done]      # NEW: force the review loop even under otherwise-silent paths
    # card DONE section renders real checks; `d` opens the review loop
```

Config: `[start] confirm_contract = false` (default). When `true`, or with
`--review-done`, the review loop runs before any interactive launch.

### Refusal cases

| Case | `try:` |
|---|---|
| critic says a keyword stub would pass, non-interactive | refuse launch → `try: deadreckon acceptance refine "<add a runtime check>"` |
| goal clause uncovered under `--yes` with strong divergence | refuse → `try: deadreckon start <goal> --review-done` |
| empty re-prompt text | keep prior draft, warn → `try: deadreckon acceptance draft "<criteria>"` |
| no provider for draft/critic | lint-only floor, warn → `try: deadreckon acceptance init` (template) |

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them fail →
implement → `make verify` green (fmt-check, clippy, public-surface, test,
build) → conventional-commit → one-line CHANGELOG entry naming the SHA.

### P1 — CompiledContract read model
- New `compile_contract` producing `CompiledContract`/`CompiledCheck` with deterministic `summary`/`behavioral`/`can_fail` classification. Pure; no UI, no provider.

Depth tests (inline `#[cfg(test)]` in `commands/acceptance.rs`):
- `compile_contract_classifies_shell_build_as_behavioral`
- `compile_contract_marks_keyword_grep_unfalsifiable`
- `compile_contract_summary_wording_is_stable`

### P2 — Goal threaded into the compiler
- `acceptance_agent_prompt` and `acceptance_agent_command_in_dir` take the run goal; `start`'s Draft invocation passes the actual goal; `acceptance draft --goal` exposes it on the CLI. The prompt gains a "Run goal" block.

Depth tests:
- `acceptance_prompt_includes_run_goal_block`
- `start_draft_passes_goal_into_acceptance_agent`

### P3 — Execution-oriented prompt rewrite
- Rewrite the prompt body to the six mandates above.

Depth tests:
- `acceptance_prompt_demands_behavioral_over_source_scan`
- `acceptance_prompt_requires_every_check_be_falsifiable`
- `acceptance_prompt_bans_if_present_only_build_test`

### P4 — Falsifiability lint (deterministic floor)
- `lint_contract(&CompiledContract) -> Vec<LintFinding>` implementing the four findings. Wired so `acceptance draft`/`init` runs it post-compile and reports findings.

Depth tests:
- `lint_flags_contract_with_no_behavioral_check`
- `lint_flags_if_present_only_build_and_test`
- `lint_flags_source_scan_as_only_substantive_gate`
- `lint_clean_on_a_build_start_assert_contract`

### P5 — Critic pass + one auto-redraft
- Critic prompt + `CriticVerdict`; one call after draft; one automatic re-draft on `Redraft`; skipped without a provider. Clamped, no loops.

Depth tests:
- `critic_redraft_fires_at_most_once`
- `critic_absent_provider_falls_back_to_lint_floor`
- `critic_flags_stub_passable_contract`

### P6 — Goal↔contract reconciliation
- `reconcile(goal, &CompiledContract) -> ContractDivergence`; deterministic clause split + coverage; folds in lint findings.

Depth tests (inline in `commands/start.rs`):
- `reconcile_reports_uncovered_realtime_clause`
- `reconcile_clean_when_every_clause_has_a_check`

### P7 — The accept / re-prompt / edit review loop
- Upgrade `prompt_start_existing_done_criteria` (and the authoring-time draft path) into a loop that renders the **real** `CompiledContract` + divergence and offers: accept, re-prompt (re-runs the compiler with goal + prior draft + note via Refine, then re-shows), edit (opens the files), check (dry-runs the gate), cancel. Re-prompt is a true loop until accept.

Depth tests:
- `review_renders_real_checks_not_just_count`
- `reprompt_recompiles_and_reshows_until_accept`
- `edit_and_check_reuse_existing_paths`

### P8 — Course card DONE section + `d`
- Expand the card's DONE block to list the compiled checks + a divergence flag; add `d` → the P7 loop before Enter sails; `--json` emits `{checks, divergence}`.

Depth tests (inline in `commands/course.rs` + `tests/cards_friendliness.rs`):
- `course_card_done_lists_compiled_checks`
- `course_card_flags_goal_divergence`
- `course_card_d_key_opens_review_loop`
- `start_json_emits_compiled_checks_and_divergence`

### P9 — Non-interactive guardrails
- Under `--yes`/non-TTY the loop is skipped but divergence still prints; strong divergence (uncovered clause AND lint failure) refuses with the `try:` lines above; `--review-done` / `[start] confirm_contract` force the loop.

Depth tests:
- `yes_launch_still_surfaces_divergence`
- `strong_divergence_refuses_under_yes_with_try`
- `review_done_flag_forces_loop_non_interactively`

### P10 — Friendliness pass
- Error-footer routing for every refusal above; `--plain`/`--quiet` parity (five-line contract summary printed as lines); post-review lifecycle hints; `?`/help grouping for the new `d` key and `--review-done`.

Depth tests:
- `plain_contract_review_prints_check_lines`
- `every_contract_refusal_emits_a_try_line`

### P11 — Architecture doc + CHANGELOG (doc only; no depth test)
- Insert `## 48. Contract: Goal-Aware, Execution-Oriented Done Criteria` into `docs/AS-BUILT-ARCHITECTURE.md` (compiled read model, prompt spec, lint + critic, reconciliation, review loop, card DONE) and cross-reference §13.1/§35 (Polyglot) and §46 (Course). Update §22 shipped list. Do not collide with Helm's §47.
- Append CHANGELOG:
  ```
  ## Contract (stable) — a definition of done you can trust — <date>
  - the done contract is compiled from the run goal, forced to test behavior
    (build/start/drive/assert; known input -> known output; every check
    falsifiable; keyword-only scans and --if-present-only gates rejected),
    checked by a deterministic falsifiability lint plus one clamped critic
    pass, and shown for accept / re-prompt / edit before launch on the Course
    card. Closes the acceptance scope-drift and stub-passable-gate gaps.
  ```
- Log Contract follow-ups in `docs/V1-CANDIDATES.md`.

## Integration matrix

| Surface | draft (authoring) | start (interactive) | start `--yes` / non-TTY |
|---|---|---|---|
| Goal into compiler | via `--goal` | automatic | automatic |
| Lint floor | yes | yes | yes (prints) |
| Critic + 1 redraft | yes (if provider) | yes (if provider) | yes (if provider) |
| Reconciliation | shown | shown + card | printed |
| Review loop | yes | `d` / `confirm_contract` | skipped unless `--review-done` |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| contract has no behavioral check | `try: deadreckon acceptance refine "add a check that builds and runs the app"` |
| goal clause uncovered | `try: deadreckon start <goal> --review-done` |
| stub would pass (non-interactive) | `try: deadreckon acceptance refine "<runtime assertion>"` |
| empty re-prompt | `try: deadreckon acceptance draft "<criteria>"` |
| no provider for compile/critic | `try: deadreckon acceptance init` |

## Config additions

```toml
[start]
confirm_contract = false   # true forces the review loop before any interactive launch
```

## Out of scope (explicitly → V1-CANDIDATES)

- New check kinds (browser-driver kind, HTTP-assert kind) — behavioral checks ride `shell` for now; a first-class kind is V1.
- A standalone `deadreckon contract` report verb (the Polyglot detect-report follow-up) — reuse `acceptance` surfaces here.
- Multi-round critic / self-repair loops — the ceiling is one critic + one redraft.
- Per-check provenance ledger (which draft authored which check) — the compiled model is ephemeral.
- Semantic (embedding-based) goal↔contract coverage — reconciliation stays deterministic keyword coverage + the single critic.
- Auto-generating a missing build/test harness in the repo — the compiler may *propose* one in `files`, but scaffolding the project is V1.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (in tree, free): serde_json (already), the existing provider router, the existing `acceptance`/`start`/`course` surfaces. Tier 2 (architectural — log to `DEPENDENCIES.md`): none expected — no new crates; behavioral checks use whatever the target project already has (npm/cargo/etc.), invoked via `shell`. Tier 3 (blocked): a bundled headless-browser dependency (Playwright/Puppeteer as a deadreckon dep) — behavioral browser checks call the *project's* tooling, deadreckon ships no runner.

## Engineering invariants (do not violate)

- **No `PipelineState` or acceptance-file schema changes.** CompiledContract is a projection.
- **The four check kinds are frozen.** Good behavior is forced by the prompt + lint, never by a new kind.
- **The lint is deterministic and is the floor.** The critic never runs without the lint, and its absence never blocks a launch the lint would allow.
- **One critic call, one redraft, no loops.**
- **The human is never silently bypassed** — divergence always surfaces even when the loop is skipped.
- **`summary` wording and card DONE layout are depth-test-pinned** — changing the phrasing changes the spec.
- **One depth test before each phase implementation.** A phase whose tests were never red is suspect.
- **No silent scope expansion** — anything beyond P1–P11 → V1-CANDIDATES.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth tests passing and a CHANGELOG entry naming the SHA.
- Provider-touching phases (P5 critic) test with a stub/fake router — no live providers in CI.
- P3 (prompt rewrite) and P7 (review loop) are the highest-risk phases; prefer two smaller commits over one big one in each.
- If a phase reveals a V1 decision (e.g. a behavioral check genuinely needs a first-class kind), stop and log it in V1-CANDIDATES; do not expand scope.
