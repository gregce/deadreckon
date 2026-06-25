GOAL: Make the acceptance gate non-hollow for non-Rust projects by auto-detecting the test contract and compiling a real default (Node, Deno, Python, Go, JVM, .NET, Ruby, PHP, Elixir, or a script-runner `test` target) instead of the current `cargo test`-or-`FileExists` fallback. Today a run with no `acceptance.yaml` in a TS/Python/Go tree gets a trivial gate (`compiled_acceptance_checks` returns `FileExists {working_dir}`), so "VERIFIED" can mean nothing was checked — a dangerous trust hole for non-Rust users. This slice adds a deterministic detector that compiles a real check set, writes it to the run's auditable acceptance path, keeps tamper coverage honest for shell test commands, and — only for unknown trees — can optionally PROPOSE a contract the operator approves first. Land this slice named Polyglot.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-06-25-1421-deadreckon-polyglot-gate-rider.md` — detection floor, inference, schemas, depth tests, tamper rules.
- `/Users/gdc/deadreckon/crates/deadreckon-core/src/gate.rs` — `AcceptanceCheck`, `compiled_acceptance_checks`, `evaluate_default_acceptance`, `acceptance_spec_path_for_run_root`.
- `/Users/gdc/deadreckon/crates/deadreckon-core/src/tamper.rs` — `CoverageClassification`, `check_coverage`, `classify`.
- `crates/deadreckon/src/commands/start.rs` + `crates/deadreckon-providers/src/narrator.rs` — goal-shape classifier + cheap-model router + deterministic-floor precedent for inference.
- `docs/AS-BUILT-ARCHITECTURE.md` §13/§35; `docs/V1-CANDIDATES.md`. Prior riders hold.

**Posture.** Stable track (0.3.1). No `PipelineState`/`AcceptanceMarker`/`AcceptanceCheck` schema breakage — existing variants (`Shell`, `CargoTest`, `FileExists`) cover it. Signed-marker contract, nonce isolation, dr-gate boundary unchanged. The floor is the default and the only thing that signs a marker unattended; inference is opt-in, approved, never silent. No `git push`. Edits inside `/Users/gdc/deadreckon`. Decisions → V1-CANDIDATES.

**Detect (deterministic floor), then optionally infer.**

- A deterministic, total, no-network detector resolves a contract by sentinels, lockfiles, and the test script/target it parses (`package.json scripts.test` + pm; script-runner `test`; `go.mod`; `pyproject`+visible tests; `Cargo.toml`). This floor signs the marker by default.
- Each kind compiles to a real `Vec<AcceptanceCheck>` kept as `Shell` (reuses evaluation/signing/tamper); the spec is written to `acceptance_spec_path_for_run_root` — auditable.
- Only when the floor is Unknown, opt-in `--infer-contract` (narrator router + deterministic fallback) PROPOSES a contract, previews it, and needs operator approval before it arms the gate — never silently, never under `--yes`/non-TTY. No approved contract → honest caveat, not green.

**Tamper stays honest cross-language.** `check_coverage`/`classify` deterministically treat shell test commands (`npm test`, `pytest`, `go test`) as `Test` coverage, so deleting/suppressing a JS/Py/Go test refuses like a deleted Rust test.

**Friendliness.** Auto-detect (floor is default). Preview the contract before the run and in `detect`. Refuse with `try:` when a kind is detected but unrunnable (→ `--acceptance`/`--infer-contract`).

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `make verify` green → conventional-commit → CHANGELOG line. P11 adds AS-BUILT §13/§35.

**Verification.**

- Every rider depth test present and passing; `make verify` green each commit.
- A fixture Node/Python/Go tree with no `acceptance.yaml` compiles and runs the real default check (not `FileExists`), writes the spec, signs over genuine results.
- Deleting a covered JS/Py/Go test refuses; inference never arms the gate without approval. No `git push`. No schema breakage.

**Stop when** verification passes, AS-BUILT + V1-CANDIDATES + a `Polyglot (stable)` CHANGELOG section are updated, committed locally.
