GOAL: Make the unforgeable gate also **tamper-evident**: a signed `dr-gate` marker is refused, or downgraded with a visible caveat, when a run modified the acceptance spec itself or a file an acceptance check covers, and the per-check verdict plus a tamper line are surfaced everywhere the outcome appears. Today `dr-gate` honestly signs a *hollow pass*: an agent can delete `tests/auth_test.rs`, or append `|| true` to a shell check, and the gate signs because the checks really do pass. The signature is real; the thing it vouches for is not. Close the only credible attack on the headline feature, and co-locate two trust-render fixes that share the gate-result path. Headline word: **Tamper-evident**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` — §13 gate, §14 provenance.
- `/Users/gdc/deadreckon/docs/goals/2026-05-28-1556-deadreckon-tamper-evident-gate-rider.md` — full contract.
- `/Users/gdc/deadreckon/crates/deadreckon-core/src/gate.rs`, `.../src/bin/dr-gate.rs`, `.../deadreckon-core/src/artifacts.rs`.
- `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs` — status + exit-card render.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` — invariants hold.

**Posture.** Production-release track; release-blocking gate behavior, not scaffolding. Files-not-fields: the tamper verdict lives in `proofs/acceptance-tamper.json`; no `PipelineState`/`Plan`/provider schema changes. The marker stays schema 1 — its signature additionally hashes the tamper file (the existing `provenance_hash` pattern), no new marker fields. Tamper-*evidence* (heuristic, surfaced), not tamper-proof. No `git push`. Edits inside `/Users/gdc/deadreckon/`. Larger-design questions → `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**User contract.**

- A run that edits `acceptance.yaml`, or whose compiled checks contain a suppression pattern (`|| true`, `; true`, `--no-verify`, `--exit-zero`), is **refused**: no marker, the gate fails non-terminally, the loop continues with a corrective reason.
- A run that modified a file an acceptance check covers (a `file_exists`/`content_match` target, or a test file a `cargo_test`/`shell` check exercises) is **signed with a caveat**, surfaced loudly, never silently passed.
- A clean run signs exactly as today.
- `status`, the exit card, and `--why-failed` show a per-check verdict (`gate: FAILED 1/4 — cargo_test x (auth::tests::expired_token)`) and `tests modified this run: yes/no`; caveats render with `Warn` tone.
- Subscription turns never print `~$0.000000`; spend reads `not metered (subscription) · wall <s>s · <n> turns`.

**Policy (decided; rider is the spec).** **Refuse** = spec edited this run or a suppression-pattern lint hit (no marker). **Caveat** = agent modified/deleted a check-covered file (modifying tested *production* code is normal; touching the *test or contract* is the danger — rider heuristics draw the line). Touched files = `provenance.jsonl` union first-snapshot vs final working-dir diff (catches deletions). Forged tamper files fail marker-signature validation.

**Phases.** Eight (P1-P8) in the rider. Each: depth test first -> implement -> focused tests green -> conventional local commit -> CHANGELOG. P1 reproduces the hollow pass (RED). P8 adds an AS-BUILT section and logs deferrals.

**Verification.**

- Every rider depth test present and passing; `cargo fmt --check`; `git diff --check`.
- Smoke: a run that deletes a covered test file is **refused** (no marker; never `Completed`).
- Smoke: a clean run **signs** and promotes as today; a caveat run signs and the exit card shows the caveat with `Warn` tone.
- No edits outside the repo; no `git push`; no `PipelineState`/`Plan` schema changes.

**Stop when** verification passes, the hollow pass is refused, caveat and per-check verdicts render, honest subscription spend renders, AS-BUILT and CHANGELOG record the behavior, deferrals are in V1-CANDIDATES, and the work is committed locally.
