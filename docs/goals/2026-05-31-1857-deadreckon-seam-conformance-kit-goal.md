GOAL: Turn composable seams into a **conformance kit**. The runtime can now swap policy, model-catalog, hooks, and event-sink workers through `[seams]`, but the worker contract is still mostly implicit in Rust tests and architecture prose. Add executable examples, validation, and docs so a user can build a seam worker without forking deadreckon. Headline word: Conformant.

Read first:
- `/Users/gdc/deadreckon/docs/goals/2026-05-31-1857-deadreckon-seam-conformance-kit-rider.md`
- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` section 39, plus the tamper-evident gate sections it references.
- `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/seam.rs`
- `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/turn_loop.rs`
- `/Users/gdc/deadreckon/crates/deadreckon/src/commands/doctor.rs`
- `/Users/gdc/deadreckon/crates/deadreckon/src/cli.rs`
- Prior riders: preserve their posture and invariants; do not reopen the gate design.

Posture:
- This is production-release work: make the feature understandable, testable, and hard to misuse.
- Do not change durable run/plan/provider schemas unless a failing test proves there is no smaller path.
- The acceptance gate is not a seam. It must remain local, deterministic, and non-swappable.
- Prefer examples, fixture validation, CLI diagnostics, and docs over new architecture.
- No push. Commit locally only after focused verification passes.

Deliver:
- Example seam workers under `/Users/gdc/deadreckon/examples/seams/` for policy allow/deny, catalog override, hooks JSONL, and event sink JSONL.
- Fixture JSON and a sample config that exercise all four seam kinds with no network dependency.
- A conformance validation surface, preferably `deadreckon seams validate <kind> --config <path> [--fixture <path>] [--json]`, or a smaller `doctor`-based surface if that fits the CLI better.
- Clear validation output that says what passed, what failed open/closed, and what the user should try next.
- A user-facing `docs/SEAMS.md` or equivalent architecture doc section that documents the worker protocol, fail policies, sandbox expectations, example config, and `--no-seams` escape hatch.
- Focused tests/goldens proving examples and diagnostics stay executable.

Phases:
- Follow the rider's eleven phases in order. Start each behavior with a depth test or fixture check, then implement the smallest code/docs needed.
- P1-P2 establish fixtures and example workers.
- P3-P6 add the validation surface for policy, catalog, hooks, and event sink.
- P7 proves the gate/proof trust root is not reachable through examples or validation.
- P8-P10 polish docs, integration smoke, and plain/JSON output.
- P11 updates AS-BUILT, CHANGELOG, and V1 candidate notes.

Verification:
- `cargo fmt --check`
- Focused `cargo test` packages touched by seam validation and runtime dispatch.
- Every committed example worker validates from the sample config.
- A deny-policy example blocks a smoke path, while `--no-seams` and built-in behavior still work.
- Malformed catalog output reports the expected fail-open behavior.
- Adversarial fixtures cannot read or mutate gate/proof artifacts.
- `git diff --check`

Stop when:
- The conformance kit is committed locally with examples, validation, docs, and tests.
- The architecture docs explain what is intentionally still out of scope.
- There are no schema migrations, no worker registry/bus, and no pushed branch.
