# deadreckon — Verdict Rider (did it actually work?)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-06-25-1422-deadreckon-verdict-goal.md`.
It supersedes nothing in prior riders (tamper-evident, uniform-surface,
stable-readiness, live-narrator); their invariants still apply. This rider
adds a single read-only verb, `verdict`, that re-verifies any run NOW and
renders one trustworthy verdict, plus an `--all` comparison across runs.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime `~/.deadreckon`.

## Posture (decided — do not redesign)

- **Maturity stays stable** (0.3.1 shipped; lands under a `Verdict` CHANGELOG section).
- **Read-only verb.** `verdict` NEVER mutates `PipelineState`, never advances a phase, never promotes, never overwrites the original signed marker. It re-runs acceptance checks through the existing evaluation path against the run's recorded working dir and reports.
- **No `PipelineState`/`AcceptanceMarker` schema changes.** The verdict result is a sidecar file, not a state field.
- **Re-verification uses the same engine as the gate.** Verdict re-runs `compiled_acceptance_checks` + `evaluate_acceptance` (the same path dr-gate uses), so a VERIFIED from `verdict` means the same thing as a gate pass. Verdict does not invent a second notion of "done".
- **Honesty about provenance.** A run with a valid signed marker is distinguished from a run verified-now-by-re-running-checks (no marker, e.g. imported). The verdict label never claims native gating that did not happen.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Cross-machine verdicts, historical verdict trends, a verdict daemon, and auto-fix-on-regress go to V1-CANDIDATES.
- **Edits stay inside `/Users/gdc/deadreckon`.**

## Data model (files, not fields)

`<run_root>/proofs/verdict-<ISO8601>.json` — a cached, append-only audit record of each `verdict` invocation. Never read back as authority (each run re-verifies live); it exists so an operator can see when a verdict was taken and what it said.

```json
{
  "schema": 1,
  "run_id": "…",
  "taken_at": "2026-06-25T…Z",
  "state": "verified | regressed | unverified",
  "had_signed_marker": true,
  "marker_valid": true,
  "checks": [ { "kind": "shell", "passed": true, "must_pass": true, "command": "go test ./...", "detail": "…" } ],
  "changed_files": { "added": 3, "modified": 7, "deleted": 0 },
  "source": "native | imported"
}
```

No new durable struct fields anywhere.

## Verdict states (the spec — match it in code)

```
enum VerdictState { Verified, Regressed, Unverified }

fn compute_verdict(had_marker: bool, marker_valid: bool, rerun_all_must_pass: bool) -> VerdictState
```

| Condition | State |
|---|---|
| valid signed marker AND re-running its checks now all-must-pass | **Verified** |
| a marker exists (valid or stale) OR checks are known to have passed before, but re-running them now fails a must-pass check | **Regressed** |
| no signed marker (imported / paused / failed run) — verdict runs the declared/compiled checks fresh | **Unverified** (+ the fresh pass/fail detail) |

- `Regressed` is the load-bearing new signal: it is what catches "the agent said done, the work later silently broke." A marker that no longer re-validates (signature mismatch, tamper bytes changed) is also `Regressed`, not `Verified`.
- `Unverified` is NOT a failure; it carries the fresh check results and is labeled "verified now, not at build time". An imported run whose checks pass now is `Unverified: checks pass`.
- Maps to `VerdictKind` in `crates/deadreckon/src/verdict_surface.rs`: Verified→the existing pass kind, Regressed→a fail kind, Unverified→a caveat kind. Reuse `VerdictSurface::try_new` (one-primary-action contract) — do not build a parallel renderer.

## Verb signatures

```
verdict [<id>|latest]
    [--all]                  # comparison table across recent runs instead of one run
    [--limit <N>]            # with --all: how many recent runs (default 10)
    [--json]                 # machine envelope; non-TTY safe
    [--plain] [--quiet]      # output policy, per existing convention
```

Refusal cases:

| Case | Behavior |
|---|---|
| no id and no runs exist | refuse with `try: deadreckon start "<goal>"` |
| unknown / ambiguous id prefix | refuse with `try: deadreckon list` (reuse `find_run_state_path` ambiguity error) |
| run still executing | verdict reports state from current checks + a note it is live; `try: deadreckon attach <id>` |
| working dir gone (e.g. exported/cleaned) | `Unverified`, detail "working dir unavailable"; `try: deadreckon show <id>` |

## Primary-action mapping (one action per verdict)

| State | Recommended command |
|---|---|
| Verified | `deadreckon finish <id>` |
| Regressed | `deadreckon resume <id>` (or `show <id> --why-failed`) |
| Unverified (checks pass) | `deadreckon finish <id>` |
| Unverified (checks fail) | `deadreckon resume <id>` |

## Phases (eleven)

Each phase: named depth test(s) **first** (watch fail) → implement → `make verify` green (fmt-check, clippy, public-surface, test, build) → conventional-commit → one-line CHANGELOG entry naming the SHA.

### P1 — Verdict module + state enum (read-only)
- New `crates/deadreckon/src/commands/verdict.rs`; define `VerdictState`, `VerdictReport`, `compute_verdict`; register the `Verdict` variant in `cli.rs` + a `main_inner` arm. No evaluation yet.

Depth tests (`crates/deadreckon/src/commands/verdict.rs` `#[cfg(test)]`):
- `compute_verdict_marker_valid_and_rerun_passes_is_verified`
- `compute_verdict_marker_present_rerun_fails_is_regressed`
- `compute_verdict_no_marker_is_unverified`

### P2 — Run resolution
- Resolve `<id>|latest|prefix` via the existing `find_run_state_path`/`list_runs`; load `PipelineState` read-only; surface ambiguity/not-found as refusals.

Depth tests:
- `verdict_resolves_latest_when_no_id`
- `verdict_unknown_id_refuses_with_try_list`
- `verdict_ambiguous_prefix_refuses`

### P3 — Live re-evaluation against recorded working dir
- Re-run checks via `compiled_acceptance_checks` + `evaluate_acceptance` against `state.working_dir`; collect full per-check results; mutate nothing.

Depth tests:
- `verdict_reruns_compiled_checks_without_mutating_state`
- `verdict_missing_working_dir_yields_unverified_detail`

### P4 — Marker read + verdict computation
- Read + `validate_acceptance_marker`; combine `had_marker`/`marker_valid` with the rerun result through `compute_verdict`; a no-longer-valid marker → `Regressed`.

Depth tests:
- `valid_marker_passing_rerun_is_verified`
- `tampered_marker_is_regressed_not_verified`
- `imported_run_without_marker_is_unverified`

### P5 — Changed-file summary
- Summarize added/modified/deleted from the run's earliest snapshot / provenance (reuse existing snapshot diff helpers); include counts in the report.

Depth tests:
- `verdict_reports_changed_file_counts`
- `verdict_changed_files_empty_when_no_snapshot`

### P6 — Single-run render via VerdictSurface
- Render label + Explanation/Evidence (per-check pass/fail, changed-file summary, provenance line) + the mapped one primary action through `VerdictSurface::try_new`.

Depth tests:
- `verdict_render_uses_one_primary_action`
- `verified_render_recommends_finish`
- `regressed_render_recommends_resume`

### P7 — `--json` parity
- Emit the inspection envelope (`{kind:"verdict", id, status:<state>, checks, changed_files, source, next_actions, paths}`) matching `commands/inspection.rs`.

Depth tests:
- `verdict_json_envelope_shape_is_stable`
- `verdict_json_includes_per_check_results`

### P8 — `--all` comparison table
- Across the most recent `--limit N` runs (or several parallel runs), one compact table: id, goal (truncated), verdict, checks pass/total, spend. `--json` → array of per-run report summaries.

Depth tests:
- `verdict_all_lists_recent_runs_with_state`
- `verdict_all_json_is_array`
- `verdict_all_respects_limit`

### P9 — Imported-run integration
- Ensure a run produced by `deadreckon import` (no native marker) flows through verdict as `Unverified` + `source:"imported"`, with fresh check results; cross-link from import's completion hint to `verdict <id>`.

Depth tests:
- `imported_run_verdict_is_unverified_imported_source`
- `import_completion_hint_points_at_verdict`

### P10 — Friendliness + cache + output policy
- Default to `latest`; cache the report to `<run_root>/proofs/verdict-<ts>.json`; honor `--quiet`/`--plain`/`--json`; never prompt; never mutate lifecycle. Add `verdict` to the help catalog (production or inspection audience) and shell completion.

Depth tests:
- `verdict_caches_report_sidecar`
- `verdict_never_mutates_run_status`
- `verdict_quiet_suppresses_secondary_actions`

### P11 — Architecture doc update + CHANGELOG (doc only; no depth test)
- Insert into `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:
  ```
  ### 13.x The verdict verb (re-verify any run)
  ### 37.x Verdict as a first-class trust surface
  ```
  documenting the three states, the re-use of the gate engine, the imported-run distinction, and the read-only guarantee.
- Update §22 "What's Built vs Scaffolding-Thin": add `verdict` to shipped; note it makes the existing gate/import/VerdictSurface composable into a glanceable post-run answer and closes the "no fast did-it-work check for unattended/parallel/imported runs" gap.
- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:
  ```
  ## Verdict (stable) — 2026-06-25
  - deadreckon verdict re-verifies any run (including imported and parallel) and reports VERIFIED / REGRESSED / UNVERIFIED with evidence and one next action; --all compares runs side by side; read-only, never mutates run state.
  ```

## Integration matrix

| Run source | Has signed marker | Verdict if rerun passes | Verdict if rerun fails |
|---|---|---|---|
| native `deadreckon run` (completed) | yes | Verified | Regressed |
| native, paused/failed | maybe | Unverified/Regressed | Regressed |
| imported (Claude Code/Codex/aider) | no | Unverified (checks pass) | Unverified (checks fail) |
| still executing | not yet | live note + current state | live note + current state |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| unknown id | `try: deadreckon list` |
| no runs exist | `try: deadreckon start "<goal>"` |
| run still executing | `try: deadreckon attach <id>` |
| working dir unavailable | `try: deadreckon show <id>` |

(Each parameterized by a depth test.)

## Out of scope (explicitly → V1-CANDIDATES)

- Auto-fix or re-drive on `Regressed` (verdict reports; it does not act).
- Historical verdict trends / a verdict dashboard / cross-machine aggregation.
- A long-lived verdict daemon or watch mode.
- Re-running checks inside a fresh sandbox (verdict reuses the gate's current execution posture; sandbox-hardening the re-run is a separate decision).
- Per-check diff blame (which edit caused a regression).

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (in tree, free): existing gate (`evaluate_acceptance`, `validate_acceptance_marker`), `verdict_surface`, `state` resolution helpers, `serde_json`. Tier 2 (architectural → DEPENDENCIES.md): none expected. Tier 3 (blocked): same as prior riders.

## Engineering invariants (do not violate)

- **Read-only.** No `PipelineState`/marker mutation, no promotion, no phase advance. A depth test asserts run status is unchanged after `verdict`.
- **One notion of done.** Re-verification reuses the gate engine; no second check evaluator.
- **No schema changes.** Verdict output is a sidecar file.
- **Provenance honesty.** Native-gated vs verified-now is always distinguished in label and JSON.
- **One depth test before each phase.** A phase whose tests were never red is suspect.
- **One verdict, one primary action** via `VerdictSurface` — no bespoke multi-action output.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth tests passing and a CHANGELOG entry naming the SHA.
- Tests construct runs via existing test helpers (real `run` fixtures + a fabricated imported run); `--all` tests use a small set of fixture runs. No live provider calls.
- If a phase reveals a V1-architecture decision, stop and log it in V1-CANDIDATES; do not silently expand scope.
