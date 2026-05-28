# deadreckon — Tamper-Evident Gate Rider (close the hollow pass)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-28-1556-deadreckon-tamper-evident-gate-goal.md`.
It supersedes nothing in prior riders (notably
`2026-05-11-2110-deadreckon-audit-harden-rider.md`,
`2026-05-26-1546-deadreckon-narrative-attach-rider.md`) — their invariants still
apply. This rider adds **tamper-evidence**: the gate refuses to sign, or signs
with a surfaced caveat, when a run modified the acceptance contract itself or a
file an acceptance check covers; and it surfaces per-check verdicts plus honest
subscription spend on every result surface.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime `~/.deadreckon/`.

## The bug this closes (read before designing)

`dr-gate` (`crates/deadreckon/src/bin/dr-gate.rs`) calls
`evaluate_acceptance_checks_with_progress(run_root, working_dir)` then
`write_acceptance_marker_with_results(...)`. The marker signature already binds
the check *results* (`gate.rs::marker_signature` hashes every
`AcceptanceCheckResult`), so a *post-hoc edit of the marker* is caught
(`marker_signature_includes_check_results`). The unclosed hole is upstream: the
agent can make the checks **honestly pass** by changing what they check.

- Default acceptance is "`cargo test` passes" when `Cargo.toml` exists
  (`gate.rs::evaluate_default_acceptance`). An agent that **deletes the failing
  test** gets a green `cargo test` and a valid signature.
- A `shell` check compiled as `pytest || true` (or `--no-verify`, `--exit-zero`)
  exits 0 forever; `evaluate_check` reports `passed: output.status.success()`.
- A `content_match`/`file_exists` check can be satisfied by the agent writing the
  literal matched string into the very file under inspection.

The signature is real; the exam was rigged. We make the rigging **visible**
(tamper-evidence) and **refuse** the unambiguous cases. This is heuristic, not a
soundness proof — say so in the docs.

## Posture (decided — do not redesign)

- **Production-release track.** Release-blocking gate behavior. Not provisional.
- **Files-not-fields.** The tamper verdict is a new file
  `<run-root>/proofs/acceptance-tamper.json`. **No** new `PipelineState`, `Plan`,
  `AcceptanceMarker`, `AcceptanceCheckResult`, or provider-config fields. The
  marker stays `schema_version: 1`; its signature gains the tamper-file digest as
  an *external hashed input* (mirrors `PromotionManifest.provenance_hash`), which
  needs no marker field.
- **Tamper-evidence, not tamper-proof.** Refuse only unambiguous cases; downgrade
  ambiguous ones to a surfaced caveat; never silently pass a tampered run.
- **Reuse the non-terminal gate-failure path.** A refusal writes no marker, so the
  existing turn-loop behavior (gate fails -> loop continues with a corrective
  reason -> turn budget bounds it) applies unchanged. Do not invent a new
  terminal state.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Larger designs (causal tamper proofs, language-aware test
  detection, signed audit log) go to `docs/V1-CANDIDATES.md`.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Data model (files, not fields)

New file `<run-root>/proofs/acceptance-tamper.json`:

```jsonc
{
  "schema_version": 1,
  "run_id": "…",
  "evaluated_at": "RFC3339",
  "verdict": "clean" | "caveat" | "refuse",
  "spec_modified": false,             // acceptance.yaml touched this run
  "lint_findings": [                  // suppression patterns in compiled checks
    { "check_kind": "shell", "command": "pytest || true", "pattern": "|| true" }
  ],
  "covered_files_touched": [          // check-covered files modified/deleted this run
    { "path": "tests/auth_test.rs", "change": "deleted",  "by_check": "cargo_test", "classification": "test" },
    { "path": "README.md",          "change": "modified", "by_check": "file_exists", "classification": "target" }
  ],
  "caveats": [ "agent modified test file tests/auth_test.rs this run" ],
  "refusal_reasons": [ ]             // non-empty iff verdict == "refuse"
}
```

`change` ∈ `modified | deleted | created`. `classification` ∈
`test | target | build | unknown` (see coverage rules). `clean` =>
`covered_files_touched`, `caveats`, `lint_findings`, `refusal_reasons` all empty
and `spec_modified` false.

## Detection (the spec — match it in code)

Put the new logic in a dedicated core module
`crates/deadreckon-core/src/tamper.rs` (re-exported from `lib.rs`); keep
`gate.rs` and `dr-gate.rs` thin callers. Pure functions, unit-testable without a
provider or sandbox.

### Touched-file set

`touched_files(run_root, working_dir) -> BTreeMap<RelPath, Change>`:

- **Modified/created**: union of every `files` entry across `provenance.jsonl`
  (`ProvenanceRecord.files`), normalized to working-dir-relative paths
  (strip the `working_dir` prefix; ignore paths outside it).
- **Deleted**: diff the **earliest** snapshot inventory
  (`<run-root>/snapshots/turn-*` lowest N, via `inventory_files`) against the
  final `working_dir` inventory. A path present in the first snapshot and absent
  now is `deleted`. (Provenance does not reliably record deletions; the snapshot
  diff is what catches the deleted-test attack.)
- Always exclude the `.deadreckon/` subtree (run-owned docs/state), so generated
  `RUN-*.md` never counts as tampering.

### Check coverage

`check_coverage(checks, working_dir) -> Vec<CheckCoverage>` maps each
`AcceptanceCheck` to the working-dir-relative paths it depends on and a
`classification`:

- `FileExists { path }` / `ContentMatch { path }` -> exact rendered path
  (`render_template`); `classification = target`.
- `BuildSuccess { cwd }` -> the `cwd` directory; `classification = build`
  (low signal — included for completeness, never by itself a caveat).
- `CargoTest { .. }` (and the default `cargo test`) -> test files under
  `working_dir`: paths matching `**/tests/**/*.rs`, `*_test.rs`, or any `.rs`
  file whose contents contain `#[test]` or `#[cfg(test)]`;
  `classification = test`.
- `Shell { command, .. }` -> (a) **lint** the command string for suppression
  patterns; (b) best-effort path extraction: whitespace tokens that resolve to an
  existing working-dir path; test-name heuristic above sets `test`, else
  `unknown`. Path extraction is best-effort; the lint is authoritative.

### Suppression-pattern lint

`lint_suppressions(command) -> Vec<Finding>` flags (case-insensitive, token-aware
so `||true` and `|| true` both hit): `|| true`, `; true`, `&& true`, `| true`,
`|| exit 0`, `--no-verify`, `--exit-zero`, `|| :`. Applies to `Shell.command`
and any `CargoTest`/`BuildSuccess` extra args. Used at **two** sites: def-done
compile time (P4, reject) and gate time for legacy/edited specs (P5, refuse).

### Verdict policy

`classify(spec_modified, lint_findings, covered_touched) -> Verdict`:

1. **refuse** iff `spec_modified` OR `!lint_findings.is_empty()`.
2. else **caveat** iff any `covered_files_touched` has `classification ∈
   {test, target}` (i.e. the agent changed a *test* or the *literal artifact a
   check inspects*). `build`/`unknown`-only touches do **not** trigger a caveat
   but are still recorded.
3. else **clean**.

Rationale to preserve in code comments: modifying *production* code that a test
*exercises* is the whole point of a run and must stay `clean`; the danger is
editing the test/assertion or the contract. The heuristic intentionally errs
toward a visible caveat over a silent pass, and toward refuse only when the
contract itself or a check's exit code was subverted.

## Gate integration (P5)

`dr-gate.rs`, after `evaluate_acceptance_checks_with_progress` and before
writing the marker:

```
let tamper = tamper::evaluate(&run_root, &working_dir, &compiled_checks)?;
write tamper -> proofs/acceptance-tamper.json   // always, all three verdicts
match tamper.verdict {
    Refuse  => { print refusal_reasons to stderr; exit nonzero; NO marker }
    Caveat  => { write_acceptance_marker_with_results(...) }  // signed, caveat on file
    Clean   => { write_acceptance_marker_with_results(...) }  // unchanged path
}
```

`gate.rs::marker_signature` gains the tamper-file digest as a hashed input:
read `proofs/acceptance-tamper.json` bytes (empty string if absent, for
backward tolerance) and `.hash()` them alongside the existing fields. Therefore
`validate_acceptance_marker` (already the promotion guard) transitively rejects
any marker whose tamper file was altered after signing — the agent cannot edit
`acceptance-tamper.json` to erase a caveat without invalidating the signature,
and cannot fabricate a Clean tamper file to accompany a marker it cannot sign.

`compiled_checks` must be the same parsed `Vec<AcceptanceCheck>` the evaluator
used (factor a `parse for run_root` helper out of `evaluate_acceptance_checks_inner`
rather than re-reading divergently).

## Render integration (P6) — `crates/deadreckon/src/main.rs`

Use existing `ui_card`/`ui` primitives and the `glossary` vocabulary; no new
palette. Surface on the exit card, `status`, and `--why-failed`:

- Per-check verdict line from `marker.checks` (data already present): e.g.
  `gate: PASSED 4/4` or `gate: FAILED 1/4 — cargo_test x (auth::tests::expired_token)`.
  Failing-check detail comes from `AcceptanceCheckResult.detail`.
- `tests modified this run: yes/no` derived from `acceptance-tamper.json`
  (`covered_files_touched` with `classification == test` non-empty).
- On `verdict == caveat`: render the caveat lines with **`Warn`** tone
  (`accepted (caveat: agent modified tests/auth_test.rs this run)`).
- On a refusal (no marker): the failure reason already flows through the gate
  path; ensure it reads e.g.
  `acceptance refused: agent modified acceptance.yaml this run` /
  `acceptance refused: suppression pattern '|| true' in shell check`.

## Honest subscription spend (P7) — co-located render fix

`SpendRecord` already carries `subscription: bool`. Wherever spend renders
(`main.rs` exit card, `status`, summaries): when the run's spend rows are all
`subscription == true` (or the resolved provider route is a subscription CLI),
**do not** print a dollar figure. Render
`spend: not metered (subscription) · wall <s>s · <n> turns` and a cap line that
names only the caps that apply on that route (wall-clock + turns). Mixed routes
(some metered rows) print the metered total plus a `+ subscription turns` note.
Never emit `~$0.000000`.

## Phases (eight)

Each phase: write the named depth test(s) **first** and watch them fail;
implement; green on `cargo test -p <touched crate>` for the touched modules plus
`cargo fmt --check`; conventional-commit local commit; one-line CHANGELOG entry.
Do not run `make verify`/release/stress/full-workspace suites unless the human
asks.

### P1 — Reproduce the hollow pass (RED)

- Depth tests in `crates/deadreckon-core/src/gate.rs` (or `tamper.rs` test mod)
  that construct a run whose default/`cargo_test` (or `file_exists`) check passes
  *only because a covered file was removed/edited*, drive the current
  `dr-gate` code path, and assert the **desired** post-state — which fails today.
- These stay green at the end as the regression guard.

Depth tests:
- `deleting_a_covered_test_file_must_not_yield_a_signed_marker`
- `editing_acceptance_yaml_during_run_must_not_yield_a_signed_marker`
- `suppression_pattern_in_shell_check_must_not_yield_a_signed_marker`

### P2 — Touched-file set + check coverage

- Implement `tamper::touched_files` (provenance union + first-snapshot deletion
  diff, `.deadreckon/` excluded) and `tamper::check_coverage` with the
  classification rules above. No verdict yet.

Depth tests (`crates/deadreckon-core/src/tamper.rs`):
- `touched_files_unions_provenance_and_detects_snapshot_deletions`
- `touched_files_excludes_deadreckon_subtree`
- `check_coverage_classifies_test_target_build_unknown`
- `cargo_test_coverage_matches_test_dirs_and_cfg_test_files`

### P3 — Verdict computation + tamper record

- Implement `tamper::lint_suppressions`, `tamper::classify`, and
  `tamper::evaluate(run_root, working_dir, checks) -> AcceptanceTamper`, writing
  `proofs/acceptance-tamper.json`. Pure; no marker interaction yet.

Depth tests:
- `spec_modified_yields_refuse`
- `suppression_finding_yields_refuse`
- `modified_test_file_yields_caveat`
- `modified_production_code_only_stays_clean`
- `build_or_unknown_touch_only_stays_clean_but_is_recorded`

### P4 — def-done compile-time suppression lint

- At def-done compile (where the CLI writes `acceptance.yaml` from English /
  packs) reject specs whose compiled checks contain suppression patterns, with an
  error footer pointing at the offending pattern. Legacy specs are caught again
  at gate time (P5).

Depth tests:
- `def_done_compile_rejects_or_true_suppression`
- `def_done_compile_rejects_no_verify_and_exit_zero`

### P5 — Wire into the gate + bind signature

- `dr-gate.rs` computes the verdict, refuses (no marker) or signs (Caveat/Clean),
  always writing the tamper file. `gate.rs::marker_signature` hashes the tamper
  file bytes. `validate_acceptance_marker` thereby enforces both refusal (no
  marker to validate) and tamper-file integrity.

Depth tests:
- `gate_refuse_writes_tamper_file_and_no_marker`
- `gate_caveat_writes_signed_marker_and_caveat_record`
- `forged_tamper_file_fails_marker_signature_validation`
- `clean_run_signs_and_validates_unchanged`

### P6 — status + exit-card surfacing

- Render per-check verdict, `tests modified this run`, and caveat (`Warn` tone)
  on exit card, `status`, and `--why-failed`.

Depth tests (`crates/deadreckon/`):
- `exit_card_shows_per_check_verdict_and_failing_detail`
- `caveat_run_renders_warn_tone_caveat_line`
- `status_shows_tests_modified_flag`

### P7 — Honest subscription spend render

- Replace `~$0.000000` for subscription routes with
  `not metered (subscription) · wall · turns`; handle mixed routes.

Depth tests:
- `subscription_only_run_renders_not_metered`
- `mixed_route_run_renders_metered_total_plus_subscription_note`

### P8 — AS-BUILT + CHANGELOG + V1-CANDIDATES (doc only; no depth test)

- New AS-BUILT section:
  ```
  ## 35. Tamper-Evident Gate

  35.1 The hollow-pass attack
  35.2 Touched-file set (provenance union + snapshot deletion diff)
  35.3 Check coverage and classification
  35.4 Verdict policy (refuse / caveat / clean)
  35.5 acceptance-tamper.json and signature binding
  35.6 Surfacing: per-check verdict, tests-modified, caveat tone
  35.7 Honest subscription spend
  35.8 Limits (heuristic, not a soundness proof)
  ```
  Update §13 (gate) and §14 (telemetry) cross-references, and the
  "what's shipped vs scaffolding-thin" list: add tamper-evidence + per-check
  verdict + honest subscription spend to the shipped side; state explicitly that
  this closes the hollow-pass gap noted by the strategy synthesis and does **not**
  claim causal/soundness guarantees.
- Append to `CHANGELOG.md`:
  ```
  ## Tamper-Evident Gate (production release) — 2026-05-28

  - Refuse to sign when a run edits acceptance.yaml or a compiled check carries a
    suppression pattern; downgrade to a surfaced caveat when a run modifies a
    check-covered test/target file; bind the tamper record into the marker
    signature.
  - Surface per-check verdicts and a tests-modified flag on the exit card,
    status, and --why-failed.
  - Render honest subscription spend (no more ~$0.000000).
  ```
- Log to `docs/V1-CANDIDATES.md`: causal tamper proof (did the edit *cause* the
  pass), language-aware test detection beyond Rust heuristics, a signed
  tamper/audit log distinct from learning audit logs, and fleet-level tamper
  reporting.

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| `acceptance refused: agent modified acceptance.yaml this run` | `deadreckon undo --run <id> --turn <n>` then re-run without editing the contract |
| `acceptance refused: suppression pattern '\|\| true' in shell check` | `deadreckon def-done` and remove the suppression; checks must fail honestly |
| `accepted (caveat: agent modified <test> this run)` | `deadreckon show <id>` to review the touched test before `deadreckon apply` |

(Each footer is exercised by a P5/P6 depth test.)

## Integration matrix

| Verdict | Marker written | Run can reach `Completed` | Exit-card tone | tamper file |
|---|---|---|---|---|
| clean | yes | yes | normal | written (clean) |
| caveat | yes (signed) | yes | `Warn` + caveat line | written (caveats) |
| refuse | no | no (gate fails non-terminally) | failure reason | written (refusal_reasons) |

## Out of scope (explicitly V1 candidates)

- Causal proof that a covered-file edit *caused* a check to pass.
- Language-aware test detection beyond the Rust `#[test]`/`tests/` heuristics.
- A separate signed tamper/audit log (distinct from `learning/` audit logs).
- Fleet/plan-level aggregate tamper reporting across many runs.
- Sandboxing the checks' own filesystem writes (a different threat model).

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (already in-tree): `walkdir` (inventory diff), `regex` (pattern lint),
`serde`/`serde_json` (tamper file), `chrono`. **No new crates expected.**
Tier 2: none. Tier 3: same blocks as prior riders.

## Engineering invariants (do not violate)

- **No `PipelineState`/`Plan`/`AcceptanceMarker`/`AcceptanceCheckResult` field
  additions.** Tamper state is the `acceptance-tamper.json` file; the signature
  hashes that file as an external input.
- **One depth test before each phase implementation.** A phase whose tests were
  never red is suspect; P1's tests prove the bug exists first.
- **Refusal reuses the existing non-terminal gate-failure path.** No new run
  status, no new terminal state.
- **Production code edits stay `clean`.** Guard this with
  `modified_production_code_only_stays_clean` — a regression here re-breaks
  normal runs.
- **No silent expansion.** Anything beyond P1–P8 goes to `V1-CANDIDATES.md`.
- **Signature determinism is the spec.** Changing what `marker_signature` hashes
  changes the contract; it is depth-tested
  (`forged_tamper_file_fails_marker_signature_validation`).

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth tests passing, `cargo fmt --check` clean, and a
  CHANGELOG entry naming the SHA.
- If a phase reveals a V1-architecture decision (e.g. causal proof tractability),
  stop and log it in `V1-CANDIDATES.md`; do not silently expand scope.
- Optional after P8: a short asciinema cast of a refused hollow pass under
  `/Users/gdc/deadreckon/` demo assets. Skip if not user-visible enough to earn it.
