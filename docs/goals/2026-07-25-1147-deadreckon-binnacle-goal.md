GOAL: Make the seal real — a receipt the agent cannot forge, and a promotion that re-derives instead of trusting. README.md:61 and CONCEPTS.md say the agent "literally cannot forge the signature". False today. The nonce is written to `run_root/gate/nonce` (`state.rs:207`) while copy/fresh runs put the agent's cwd at `run_root/working` (`state.rs:265`) — a sibling, one `cat ../gate/nonce` away. Nothing denies it: `turn_loop.rs:753` and `cli_common.rs:92` both pass `read_denylist: Vec::new()`, and the Seatbelt profile opens `(allow default)` (`commands.rs:195`). The signature is `DefaultHasher` (`gate.rs:755`) — unkeyed SipHash-1-3, 64 bits, field order published in this repo. And `promote_completed_run` (`promotion.rs:32`) validates the stored marker without re-running `dr-gate`. An agent reads the key, reproduces the stamp in thirty lines, and `finish` promotes work that never passed. The live loop is sound and is not what breaks — `turn_loop.rs:2112` feeds gate failure back into history and continues; the durable evidence breaks. Land this slice named Binnacle.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-07-25-1147-deadreckon-binnacle-rider.md` — key store, MAC, marker v2, containment, staleness rule, eleven phases, depth tests.
- `gate.rs` (:122 nonce path, :126 validate, :752 signature), `state.rs` (:207 nonce write, :255 `working_dir_for_codebase`), `promotion.rs` (:32).
- `runtime/src/seam.rs` (:324 `seam_denied_paths` — mechanism exists, not wired to agent paths).
- `core/src/tamper.rs` — 1,206 lines defending a *different* attack class. Preserve it; its inputs need protecting too.
- `docs/MAP-OF-DEADRECKON.md` "the gate ... strongest claim is not yet proven". Prior riders hold; Binnacle takes AS-BUILT §57 (§53/§54 hold for Capstan/Drydock).

**Posture.** Stable track. The live turn loop is unchanged. No new verbs; one new flag (`--yes-uncontained`). `AcceptanceMarker` gains `#[serde(default)]` fields and `schema_version` goes to 2; **v1 markers keep validating through a legacy path** so existing runs stay readable, labelled weak rather than silently trusted. No `git push`. Edits inside `/Users/gdc/deadreckon`. Decisions → `docs/V1-CANDIDATES.md`.

**The four decisions (settled — do not redesign).**

- **Key lives outside the run root**: `~/.deadreckon/gate-keys/<run-id>.key`, mode 0600, handed to `dr-gate` by env. Not a sibling of the agent's cwd, so a forgotten denylist cannot re-expose it.
- **`finish` re-runs the gate when the tree changed since the marker.** Unchanged trees promote on the stored receipt; changed trees re-derive.
- **`--sandbox none` records `contained: false`** and needs `finish --yes-uncontained` to promote.
- **README/CONCEPTS corrected in the same commit that makes the claim true** (P11), so docs are never ahead of code.

**The receipt must bind** nonce, marker fields, tamper verdict and campaign roll-up as today, plus the containment fact and resolved backend — otherwise containment is itself forgeable.

**Phases.** Eleven (P1–P11) in the rider. Each: named depth tests first, watched red → implement → `make verify` green → conventional commit → CHANGELOG line naming the SHA.

**Verification.**

- `make verify` green on every commit; every rider depth test present and passing.
- **Hostile-agent test per backend**: a process with the agent's privileges tries to read the key and write a validating marker. Must fail on seatbelt/bwrap/docker; under `none` the receipt must be marked uncontained.
- A v1 marker still validates and reports as legacy/weak, not verified.
- `finish` on an unchanged tree does not re-run checks; on a changed tree it does.
- The live turn loop is untouched: gate failure still appends to history and continues.

**Stop when** verification passes, AS-BUILT has §57, README and CONCEPTS state only what the code enforces, CHANGELOG has a "Binnacle (stable)" section naming each phase SHA, and the work is committed.
