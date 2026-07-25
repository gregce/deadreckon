# deadreckon — Binnacle Rider (make the seal real)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-07-25-1147-deadreckon-binnacle-goal.md`.
It supersedes nothing in prior riders — their invariants still apply. This
rider adds: a **key store outside the run root**, an **HMAC-SHA256 marker
signature** at `schema_version: 2` with a legacy path for v1, **containment
recorded and bound into the signature**, **denylists wired to every agent
execution path**, a **staleness rule at promotion**, and a **hostile-agent test
per sandbox backend** that is the whole point of the slice.

A binnacle is the housing that protects a ship's compass. This slice protects
the instrument DeadReckon navigates by.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime `~/.deadreckon`.

## Posture (decided — do not redesign)

- **Maturity stays stable** (lands under a `Binnacle` CHANGELOG section).
- **The live turn loop is untouched.** `turn_loop.rs:2112` appends the gate
  failure to the agent's history and returns `Ok(false)`; the caller `continue`s.
  That mechanism is correct and is not what this slice fixes. If a phase seems
  to want to change how a run decides to keep going, it is out of scope.
- **`tamper.rs` is preserved in full.** Its 1,206 lines defend the *check-gaming*
  attack (deleted tests, edited spec, suppression patterns, coverage
  reconstructed from the earliest snapshot). That is the product's strongest
  real differentiator. This slice protects its **inputs**; it does not touch its
  logic.
- **v1 markers keep validating.** Existing receipts were signed with
  `DefaultHasher`. They validate through an explicit legacy path and are
  reported as legacy/weak. Deleting the ability to read old receipts would
  destroy the evidence property the product exists for.
- **One new flag only:** `finish --yes-uncontained`. No new verbs.
- **No `git push`.** Phased local commits. **No V1 invention.**
- **Edits stay inside `/Users/gdc/deadreckon`.**

## What is actually broken (verified, do not re-derive)

| Fact | Evidence |
|---|---|
| Key is a sibling of the agent's cwd | `state.rs:207` writes `run_root/gate/nonce`; `state.rs:265` puts copy/fresh working dirs at `run_root/working` |
| Nothing denies it to the agent | `turn_loop.rs:753`, `cli_common.rs:92` both `read_denylist: Vec::new()` |
| macOS reads are unrestricted regardless | `commands.rs:195` profile opens `(allow default)`; the read allowlist above the denies is decorative |
| Signature is not a MAC | `gate.rs:755` `DefaultHasher::new()` — unkeyed SipHash-1-3, 64-bit, `{:016x}` |
| Promotion trusts the stored string | `promotion.rs:32` calls `validate_acceptance_marker` and never re-runs `dr-gate` |
| The mechanism to fix it already exists | `seam.rs:324` `seam_denied_paths` denies `gate/` and `proofs/` — to seam workers only |

The live loop is **not** in this table. It works.

## Key store

```
~/.deadreckon/gate-keys/<run-id>.key      // 32 random bytes, hex, mode 0600
```

- Written by `create_run`, replacing the `state.rs:207` nonce write.
- Handed to `dr-gate` as `DEADRECKON_GATE_KEY` from the trusted parent
  (`run_acceptance_gate`, `turn_loop.rs:2088`). `dr-gate` reads the env var and
  never needs the path, so the file is readable only by the supervisor.
- Removed by `cleanup` / `abandon` alongside the run's other durable state.
  A leaked key file for a deleted run is a loose end; test that it is gone.
- **Resume must work.** The key persists across supervisor restarts by design —
  that is why it is a file and not process memory. Validation after a restart
  reads it from the key store.
- **Missing key is a refusal, never a pass.** If the key is absent for a run
  whose marker is v2, validation fails loudly with a `try:` pointing at
  `deadreckon verdict <id>`. Do not fall back to unsigned acceptance.

## Signature

```rust
// schema_version 2
HMAC-SHA256(key = gate_key_bytes, msg = canonical_marker_bytes)
```

`canonical_marker_bytes` binds, in a fixed documented order: schema_version,
run_id, status, produced_by, checked_at (RFC3339), working_dir, check_count,
each check, the tamper file bytes, the campaign roll-up bytes (all as today) —
**plus `contained` and `sandbox_backend`**. Containment that is not bound is
containment that can be edited afterwards.

Dependency: `hmac` (RustCrypto). `sha2` is already a workspace dependency in
core, runtime and the CLI. Tier 2 — log in `DEPENDENCIES.md`.

Constant-time comparison for validation (`subtle`, or `hmac`'s own
`verify_slice`). A timing-variable `==` on a MAC is a smaller hole than the one
being closed, but it is still a hole and reviewers will look for it.

## Containment

`AcceptanceMarker` gains, both `#[serde(default)]`:

```rust
pub contained: bool,            // false when the resolved backend was `none`
pub sandbox_backend: String,    // the backend that actually ran, post-fallback
```

The **resolved** backend, not the requested one: `sandbox/src/lib.rs` falls back
to `None` with a warning when a native backend is unavailable, and
`SandboxFact` in `run_view.rs` already carries `backend` and `fallback_note`.
A run that asked for seatbelt and silently got `none` must record `none`.

## Promotion staleness rule

`promote_completed_run` (`promotion.rs:32`) currently validates and proceeds.
It becomes: validate → decide whether the tree moved since `checked_at` → if it
moved, re-run `dr-gate` and re-validate before promoting.

"Moved" is decided by the same snapshot machinery the tamper layer already uses,
not by a new mechanism. Cheap path first: if no working-tree file has an mtime
newer than `marker.checked_at`, promote on the stored receipt. This must not
become a second definition of "changed" — reuse what `tamper.rs`/`artifacts.rs`
already compute.

## Phases (eleven)

Each phase: write the named depth tests **first** and watch them fail;
implement; `make verify` green; conventional-commit local commit; one-line
CHANGELOG entry naming the SHA.

### P1 — Key store

- `gate_key_path(paths, run_id)`, create with 0600, read/write helpers.
- `create_run` writes there instead of `run_root/gate/nonce`.
- `cleanup`/`abandon` remove it.
- Signing still uses `DefaultHasher` — this phase only moves the secret.

Depth tests (`crates/deadreckon-core/src/gate.rs`):
- `gate_key_is_written_outside_the_run_root`
- `gate_key_file_is_owner_read_write_only`
- `gate_key_is_removed_when_the_run_is_cleaned_up`
- `missing_gate_key_refuses_validation_rather_than_passing`

### P2 — `dr-gate` receives the key by env

- `run_acceptance_gate` sets `DEADRECKON_GATE_KEY`; `dr-gate` reads it.
- The gate no longer resolves the key path itself.

Depth tests:
- `dr_gate_signs_from_the_env_key_without_reading_the_key_path`
- `dr_gate_without_the_env_key_fails_loudly`

### P3 — HMAC-SHA256 at schema_version 2, legacy path for v1

- Add `hmac`; log in `DEPENDENCIES.md`. Constant-time verify.
- `validate_acceptance_marker` dispatches on `schema_version`.

Depth tests:
- `v2_marker_signature_is_hmac_sha256_over_the_canonical_bytes`
- `v1_marker_still_validates_through_the_legacy_path`
- `v1_marker_is_reported_as_legacy_not_verified`
- `signature_comparison_is_constant_time`
- `marker_bytes_are_canonical_and_field_order_is_pinned`

### P4 — Containment recorded and bound

- `contained` and `sandbox_backend` on the marker, from the **resolved** backend.
- Both bound into the signature.

Depth tests:
- `marker_records_the_resolved_backend_not_the_requested_one`
- `sandbox_fallback_to_none_records_contained_false`
- `editing_contained_after_signing_invalidates_the_marker`

### P5 — Denylists on every agent execution path

- `turn_loop.rs:753` and `cli_common.rs:92` receive the denied set.
- Extend beyond `gate/` and `proofs/` to `snapshots/` and `provenance.jsonl` —
  the tamper layer's inputs. Protecting the seal while leaving the evidence it
  is derived from writable is half a fix.
- Factor the denied-path list so seams and agents share one definition.

Depth tests (`crates/deadreckon-runtime/`):
- `agent_bash_tool_denies_the_gate_and_evidence_paths`
- `cli_provider_denies_the_gate_and_evidence_paths`
- `seam_and_agent_share_one_denied_path_definition`

### P6 — Seatbelt deny-by-default

- Replace `(allow default)` with an explicit allow set.
- This is the riskiest phase in the slice: an over-tight profile breaks real
  provider runs. Land it with the smoke suite and `preflight-real.sh` in mind,
  and keep the previous profile reachable behind a documented escape hatch only
  if a real failure demands it.

Depth tests (`crates/deadreckon-sandbox/`):
- `seatbelt_profile_denies_by_default`
- `seatbelt_profile_still_permits_the_working_directory`
- `seatbelt_profile_denies_the_gate_key_store`

### P7 — Hostile-agent test per backend

**This is the phase the slice exists for.** A process with exactly the agent's
privileges, in the agent's cwd, attempts in order: read the key store; read
`run_root/gate`; write a marker that validates; edit `contained` in a signed
marker; delete a snapshot the tamper layer depends on.

- seatbelt / bwrap / docker: every attempt must fail.
- `none`: attempts may succeed, and the receipt must be marked
  `contained: false` — the honest outcome, asserted rather than hoped.
- Backends unavailable on the host skip **loudly** (a skipped backend is
  reported, never silently counted as a pass).

Depth tests (`crates/deadreckon/tests/hostile_agent.rs`, new):
- `hostile_agent_cannot_read_the_gate_key_under_<backend>`
- `hostile_agent_cannot_forge_a_validating_marker_under_<backend>`
- `hostile_agent_cannot_edit_containment_after_signing`
- `hostile_agent_cannot_delete_tamper_evidence_under_<backend>`
- `uncontained_backend_is_recorded_rather_than_pretending`
- `unavailable_backends_are_reported_not_silently_skipped`

### P8 — Promotion re-derives when the tree moved

- The staleness rule above, in `promote_completed_run`.

Depth tests (`crates/deadreckon-core/src/promotion.rs`):
- `unchanged_tree_promotes_on_the_stored_receipt`
- `changed_tree_reruns_the_gate_before_promoting`
- `rerun_that_fails_refuses_promotion_with_a_runnable_try`
- `staleness_reuses_the_existing_change_detection`

### P9 — `finish --yes-uncontained`

- Promoting a run whose receipt says `contained: false` refuses without the
  flag, with a refusal that explains rather than scolds.

Depth tests (`crates/deadreckon/tests/lifecycle.rs`):
- `finish_refuses_an_uncontained_run_without_the_flag`
- `finish_promotes_an_uncontained_run_with_the_flag`
- `refusal_names_the_flag_and_says_what_it_means`

### P10 — Surface the truth

- `verdict`, `show`, `report` and `RunView` state containment and signature
  strength plainly. A v1 receipt reads as legacy; an uncontained receipt says
  it attests the checks ran, not that the agent could not have forged it.
- `--json` parity for every new field.

Depth tests (`crates/deadreckon/tests/coherence.rs`, `verdict_surface.rs`):
- `verdict_states_containment_for_an_uncontained_run`
- `verdict_labels_a_v1_receipt_as_legacy`
- `containment_and_signature_strength_appear_in_json_and_human_output`

### P11 — AS-BUILT §57, CHANGELOG, and the doc correction (doc only)

- Insert `## 57. Binnacle: A Receipt That Cannot Be Forged` with subsections for
  the key store, the MAC and marker v2, containment binding, the denied-path
  set, the promotion staleness rule, the hostile-agent matrix, and V1 boundaries.
- **Correct `README.md:61` and `docs/CONCEPTS.md`.** Only now — the code makes
  the claim true in P1–P9, so the docs stop being ahead of it here. State what
  is enforced, name the `none` caveat, and keep the tamper claim prominent: the
  agent cannot quietly pass by weakening the contract.
- `## Binnacle (stable)` CHANGELOG section naming each phase SHA.

## Out of scope (explicitly → V1-CANDIDATES)

- **Keychain / OS secret storage.** A 0600 file outside the run root closes the
  attack in the goal. Keychain integration is a platform-specific follow-on.
- **Different-UID or privileged gate execution.** The honest bound of this slice
  is a same-UID agent constrained by the sandbox. Defeating an agent that can
  `ptrace` the supervisor needs privilege separation, which is a different slice.
- **Signing the whole run ledger.** Binnacle signs the acceptance receipt. A
  chain of custody over every ledger line is a protocol change.
- **Rules-as-gate and skill/hook provenance in the signature.** Adjacent and
  wanted (they share the sidecar seam), but they are their own slice.
- **Re-running the gate on every `finish` regardless of staleness.** Decided
  against: the staleness rule is the agreed shape.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 2 (log in `DEPENDENCIES.md`): `hmac` — RustCrypto, pairs with the `sha2`
already in the workspace. Optionally `subtle` for constant-time comparison if
`hmac`'s own verify is not used.

Tier 1: none expected. Tier 3: same blocks as prior riders.

## Engineering invariants (do not violate)

- **The live turn loop is not modified.** Gate failure still feeds back and
  continues.
- **`tamper.rs` logic is not modified.** Only its inputs gain protection.
- **One depth test before each phase implementation.** A phase whose tests were
  never red is suspect.
- **A missing or unreadable key refuses. It never degrades to accepting.**
- **Containment is bound into the signature**, or it is decoration.
- **Backends that cannot be tested are reported, never silently skipped.** A
  hostile-agent suite that quietly tests nothing is the exact failure mode this
  slice exists to remove.
- **No silent expansion.** Anything beyond P1–P11 goes to `V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth tests passing, `make verify` green, and a
  CHANGELOG entry naming the SHA.
- P6 (deny-by-default Seatbelt) and P7 (hostile-agent) are the phases most
  likely to surface real breakage. Run `make smoke` after each, and treat a
  provider run that stops working as a finding, not an obstacle.
- After P11, re-run `release/preflight-real.sh` before the next release: this
  slice changes sandbox behavior on the path real providers execute on.
- If a phase reveals a V1 architecture decision, log it in `V1-CANDIDATES.md`
  and continue — do not expand scope.
