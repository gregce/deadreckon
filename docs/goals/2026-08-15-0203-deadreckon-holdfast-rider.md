# deadreckon — Holdfast Rider (one candidate passes and ships)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-08-15-0203-deadreckon-holdfast-goal.md`.
It supersedes none of Watchkeeper or Soundings; their authority, durability,
source-admission and cleanup invariants still apply. Holdfast closes the
verified-result projection seam they leave between the mutable worker tree,
gate, semantic judge, receipt and promotion.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime
`$DEADRECKON_HOME` or `~/.deadreckon`.

## Posture (decided — do not redesign)

- **Maturity stays stable.** This is completion-authority correctness and
  greenfield friendliness, not a build system.
- **One candidate identity.** Candidate tree `C`, tree hash `H` and projection
  hash `P` are the sole result identity from deterministic verification onward.
- **Two policies remain distinct.** Admission capture stays immutable and
  source/provenance-oriented. Result projection is frozen only after worker
  quiescence and is delivery-oriented.
- **Late ignores are proposals, never proof.** Read project-local
  `.gitignore`/`.ignore` only. Never read late global Git config or
  `.git/info/exclude`. Admission-tracked paths cannot be removed.
- **No runtime-output allowlist at the result boundary.** Known names may
  remain temporarily for snapshots/recovery, but `C/H/P`, gate, receipt and
  promotion must not depend on `target`, `.next`, `.venv`, `dist`, or any
  other ecosystem name.
- **No gate reduction.** Compile the approved definition of done unchanged,
  execute every must-pass check under the same strict containment, then run
  the fresh independent semantic judge. Classification never waives a check.
- **No live recapture after sealing.** Verification may use disposable copies;
  promotion publishes the sealed projection or a byte-identical rematerialization.
- **Files, not fields.** Add controller-owned files below the run root. Do not
  add `PipelineState`, `Job`, launch-plan or authority fields. One additive,
  defaulted receipt field is allowed to bind `P` without invalidating old receipts.
- **No push, release, live provider/service mutation, or edits outside
  `/Users/gdc/deadreckon`.**

## Named exemplars and intentional divergence

Bazel constructs an execution root from declared inputs and keeps action
outputs in sandbox-local writable space before moving only known artifacts:
<https://bazel.build/docs/sandboxing>. Nix derivations declare precise inputs
and outputs and build in a fresh directory:
<https://releases.nixos.org/nix/nix-2.33.0/manual/store/derivation/outputs/>.
Git defines ignored files as intentionally untracked and keeps tracked files
unaffected: <https://git-scm.com/docs/gitignore>.

Extract the input/output separation, fresh execution and exact publication.
Intentionally diverge because arbitrary DeadReckon projects do not declare a
complete build graph: ignore/declaration data proposes `C`, while the complete
deterministic and semantic proof chain decides whether `C` satisfies the goal.
Tracing, overlays and build APIs may later corroborate intent; they never own it.

## Durable files and identity

Under `<run-root>/result-projection/`:

```text
policy.json          final project-local ignore proposal + admission hydration
manifest.json        sealed projection identity and omission evidence
candidate/           controller-owned clean C
evaluation/          disposable gate materialization; absent outside gate work
```

`manifest.json` schema 1:

```json
{
  "schema_version": 1,
  "run_id": "same durable Job/run id",
  "sealed_at": "RFC3339",
  "source_working_dir": "/controller-observed/path",
  "admission_policy_sha256": "sha256:...",
  "projection_policy_sha256": "sha256:...",
  "tree_sha256": "sha256:...",
  "included_files": 42,
  "included_bytes": 12345,
  "omissions": [],
  "omissions_truncated": 0
}
```

The manifest is atomically written only after materialization and a second
index of `candidate/` matches the source projection. `P` is SHA-256 of canonical
manifest bytes with no self-hash field. The receipt adds optional
`result_projection_sha256`; absence is accepted only for historical receipts.
New strict receipts require it.

## Projection algorithm

At the finalization boundary, after provider descendants are reaped and before
trusted Git staging:

1. Load the immutable admission `WorkspaceCapturePolicy` and require its
   controller-frozen Git hydration.
2. Discover only regular, bounded project-local `.gitignore` and `.ignore`
   files from the final worker tree. Freeze their literal lines, origins and
   bases. Do not execute Git or build tools.
3. Create a result policy using those final local rules plus the admission
   tracked-path set/HEAD/index identity. It has no generated-output roots.
4. Traverse with a dedicated `ResultCandidate` projection. Exclude Git control,
   DeadReckon evidence-only state and ignored untracked paths. Include
   admission-tracked paths even when late rules match. Preserve root lifecycle
   metadata needed by delivery.
5. Require a complete bounded capture. Materialize into a preparing directory,
   index source and copy, compare tree hash/file count/bytes, then atomically
   publish `candidate/` and `manifest.json`.
6. A later provider revision creates a new projection epoch by recoverably
   replacing the unverified candidate. Once marker signing begins, mutation of
   policy, manifest or candidate is a hard refusal, never an implicit refresh.

The exact selection precedence is: admission-tracked path; project-local
ignore/negation semantics; otherwise include. Operator-authored future exact
includes may be added in a later schema; this slice uses ordinary Git negation
and non-ignored paths rather than inventing a second user DSL.

## Proof and promotion binding

- **Trusted Git staging:** stage literal paths from the frozen result policy,
  plus deleted admission-tracked paths. Never broad-stage ignored runtime output.
- **Deterministic gate:** materialize `candidate/` to `evaluation/`, point the
  gate state clone there, and run the existing compiled contract unchanged.
  After cleanup, re-index `candidate/` and require `H`; re-project `evaluation/`
  and require `H`, proving checks did not modify candidate paths. Remove
  `evaluation/` after its process authority is gone.
- **Marker:** include the exact persisted result-projection manifest bytes in
  version-2 canonical HMAC input when present. Historical markers with no
  projection retain their old canonical bytes.
- **Semantic judge:** use a state clone rooted at clean `candidate/`. Add the
  manifest and omission evidence to its bounded evidence pack. Input freshness
  therefore binds judgment to `H/P`, marker, goal, contract and diff.
- **Receipt:** recompute the projection at the requested result root, require
  `H`, require the marker and judgment bound to `P`, and sign
  `result_projection_sha256 = P` with existing receipt fields.
- **Promotion:** materialize from `candidate/` with the persisted result policy,
  add only controller lifecycle metadata already excluded by receipt hashing,
  publish atomically, and re-index the library through the same policy. Require
  `H/P` before and after rename.
- **TOCTOU:** mutation of an included byte, mode, symlink target, ignore rule,
  manifest, candidate or published payload fails. Extra gate-created paths are
  disposable only when the frozen result policy excludes them.

## Failure semantics

- Required source hidden by a late ignore: clean gate or semantic review must
  revise/refuse; no receipt exists.
- Partial/over-budget/unstable projection: record exact omission evidence and
  classify as operator review required. Do not consume identical generic retries.
- Missing/corrupt projection after a new strict marker: corrupt completion,
  never legacy fallback.
- Historical Job/receipt without projection: validate under historical rules;
  never silently manufacture a new projection from its live tree.
- Intentional generated deliverable: leave it non-ignored or use a Git negation;
  it enters `C` and must pass all normal checks.

## Phases (eleven)

Each phase writes the named depth tests first and watches them fail. Then
implement, run focused tests, format/lint, make a conventional local commit and
add a CHANGELOG line naming the commit. Run the full release verification chain
at P5, P9 and P11.

### P1 — Characterize the projection seam

- Pin that gate, semantic evidence, receipt hashing and promotion currently use
  independently derived live/frozen projections.
- Add fixtures for late ignores and unknown output names without behavior change.

Depth tests:
- `late_project_ignore_is_not_yet_a_verified_result_boundary`
- `receipt_and_promotion_currently_can_select_different_trees`
- `unknown_framework_output_has_no_builtin_name`

### P2 — Result projection policy and schema

- Add `ResultCandidate`, result-policy construction and manifest wire type.
- Freeze project-local late ignore rules with admission hydration and no output roots.

Depth tests:
- `result_policy_uses_final_local_ignores_and_admission_tracked_paths`
- `result_policy_refuses_late_global_and_git_exclude_authority`
- `result_candidate_does_not_consult_runtime_root_names`
- `tracked_path_wins_over_late_ignore`

### P3 — Seal and validate one candidate

- Materialize source→preparing→candidate atomically and persist `H/P`.
- Add strict read/validate/reseal behavior and omission evidence.

Depth tests:
- `sealed_candidate_source_copy_and_manifest_share_one_tree_hash`
- `candidate_byte_mode_and_symlink_mutation_refuse`
- `projection_policy_or_manifest_mutation_refuses`
- `partial_projection_never_seals`

### P4 — Trusted Git stages the result projection

- Prepare projection before the final trusted commit.
- Stage exact projected paths and deleted tracked paths.

Depth tests:
- `late_ignored_churning_output_never_reaches_git_add`
- `deleted_tracked_path_is_staged_from_admission_identity`
- `final_git_commit_and_candidate_select_the_same_paths`

### P5 — Gate runs on a disposable exact materialization

- Run existing deterministic checks against `evaluation/` cloned from `C`.
- Require candidate and reprojected evaluation identity after cleanup.
- Bind manifest bytes into marker HMAC without breaking historical markers.

Depth tests:
- `strict_gate_evaluates_clean_candidate_not_live_worker_tree`
- `gate_random_output_is_discarded_and_not_candidate_identity`
- `gate_edit_to_candidate_path_refuses_after_checks`
- `projection_mutation_invalidates_new_marker_but_not_historical_marker`

### P6 — Semantic judgment sees the same clean candidate

- Root semantic evidence and read-only guard at `candidate/`.
- Include bounded manifest/omission evidence in the input hash.

Depth tests:
- `semantic_judge_reads_candidate_and_projection_omissions`
- `semantic_input_hash_changes_with_h_or_p`
- `live_worker_residue_is_absent_from_semantic_evidence`
- `semantic_judge_mutation_guard_covers_candidate`

### P7 — Receipt binds H and P

- Add defaulted receipt projection digest and require it for new strict results.
- Recompute using the persisted policy at each requested result root.

Depth tests:
- `new_strict_receipt_binds_candidate_tree_and_projection`
- `result_or_projection_mutation_invalidates_receipt`
- `historical_receipt_without_projection_keeps_historical_validation`
- `receipt_cannot_hash_live_rules_after_seal`

### P8 — Promotion publishes only the sealed projection

- Stage from `candidate/`, not the mutable worker tree.
- Validate `H/P` before/after atomic publication and through crash recovery.

Depth tests:
- `promotion_payload_is_exact_sealed_candidate`
- `promotion_never_recaptures_live_worker_ignores`
- `published_tree_rehash_must_equal_receipt_h`
- `crash_recovery_retains_candidate_projection_identity`

### P9 — Universal greenfield and adversarial matrix

- Cover late Next, Python and invented framework output with no runtime names.
- Cover intent ambiguity and hidden required sources.

Depth tests:
- `greenfield_next_late_ignore_promotes_without_next_allowlist`
- `greenfield_python_arbitrary_cache_name_promotes_without_registry`
- `unknown_framework_churning_lock_is_omitted_by_final_ignore`
- `same_dist_name_can_be_ignored_or_delivered_by_project_intent`
- `late_ignore_hiding_required_new_source_cannot_verify`

### P10 — Compatibility, recovery and operator surfaces

- Preserve old Jobs/receipts and Graph/Campaign/parent-repair flows.
- Report projection ambiguity as review-required evidence with one recovery hint.

Depth tests:
- `active_pre_holdfast_job_uses_frozen_historical_rules`
- `graph_campaign_and_parent_repair_keep_two_key_completion`
- `ambiguous_projection_stops_needs_review_not_retry_exhausted`
- `status_and_show_expose_candidate_and_omission_identity`

### P11 — Architecture, CHANGELOG and operator handoff

- Add AS-BUILT §60 “Holdfast: One Candidate Passes and Ships”; update §§13,
  35, 58 and 59 where live-workspace wording is obsolete.
- Update built-vs-thin and remove only the result-boundary output-name claim.
- Add `## Holdfast (stable)` to CHANGELOG with commit span.
- Write `docs/HOLDFAST-OPERATOR-ACCEPTANCE.md` with greenfield unknown-output,
  intentional `dist/`, hidden-source refusal and receipt/promotion inspection.

## Integration matrix

| Boundary | Root | Policy | Required identity |
|---|---|---|---|
| Worker turns | mutable working tree | admission capture | recoverable evidence only |
| Trusted final Git commit | mutable working tree | frozen result policy | paths equal `C` |
| Deterministic checks | disposable `evaluation/` | frozen result policy | reprojected `H` |
| Semantic judge | clean `candidate/` | frozen result policy | `H/P` in evidence hash |
| Receipt | requested result root | frozen result policy | `H/P` + both proofs |
| Promotion | `candidate/` → library | frozen result policy | `H/P` before and after |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| Projection partial or unstable | stop build/dev processes, add project-local ignore rules, then resume with a new attempt |
| Required source omitted | revise `.gitignore` or add a negation, then continue the same Job |
| Projection changed after gate | continue the Job so all checks rerun on a newly sealed candidate |
| Historical Job has no projection | inspect under its historical receipt; create a new Job for Holdfast semantics |
| Candidate/published digest mismatch | `deadreckon show <id> --why-failed` |

## Out of scope

- A universal semantic classifier for “generated”; it cannot exist for an
  intentional prebuilt artifact with identical name/bytes/history.
- Mandatory syscall tracing, Endpoint Security, minifilters, ptrace, fanotify,
  FSEvents, USN journals, Docker or OverlayFS.
- Build-system-specific output adapters. BSP/Bazel/Nix/CMake data may later add
  evidence but cannot become authority.
- A second public ignore DSL or output-name registry.
- Deleting ignored bytes from the user's workspace.
- Weakening, skipping or replacing any definition-of-done or semantic check.

## Dependencies

Tier 1 (existing): `ignore`, `serde`, `sha2`, `tempfile`, workspace capture,
artifact indexes, HMAC marker/receipt code, process-boundary cleanup.

Tier 2: none expected. If exact materialization cannot reuse capture primitives,
record the gap in `DEPENDENCIES.md` before adding a crate.

Tier 3 (blocked): kernel drivers/extensions, a bundled build daemon, remote CAS,
workflow engines, or a framework-output database.

## Engineering invariants

- One `C/H/P` from final staging through publication.
- Admission-tracked paths cannot be hidden by late ignores.
- Late host-global and Git-private ignores are never delivery authority.
- Result selection does not consult ecosystem output names.
- Gate writes never flow into the sealed candidate.
- The complete approved contract runs unchanged and contained.
- Semantic judgment is fresh, read-only and candidate-bound.
- Marker, receipt and promotion all fail on projection drift.
- Historical proofs validate only under their historical algorithm.
- No `PipelineState`, `Job`, launch-plan or authority schema expansion.
- No silent retry loop for unchanged projection ambiguity.

## Process invariants

- Phased local commits only; never stage the user's `.specstory/history` files.
- A phase whose named depth tests were not observed red is incomplete.
- Focused tests after each edit; full release verification at P5, P9 and P11.
- No network or live provider is required by tests.
- Every buildable milestone ends with a plain-language operator test script.
- Anything beyond P1–P11 goes to `docs/V1-CANDIDATES.md`; do not silently
  expand Holdfast into a build system or operating-system tracer.
