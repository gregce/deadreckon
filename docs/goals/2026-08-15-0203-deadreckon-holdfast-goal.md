GOAL: Land Holdfast: replace framework-name output guessing at the verified-result boundary with one controller-sealed result projection, so a greenfield agent may add ordinary project ignore rules and DeadReckon can omit arbitrary build/runtime output without weakening the compiled definition of done, independent semantic judgment, signed receipt, or exact promotion. The same candidate tree must be tested, judged, signed and shipped.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-08-15-0203-deadreckon-holdfast-rider.md`.
- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` §§13, 35, 48, 58 and 59.
- `/Users/gdc/deadreckon/docs/goals/2026-07-28-2321-deadreckon-watchkeeper-rider.md` and `/Users/gdc/deadreckon/docs/goals/2026-08-02-0016-deadreckon-soundings-rider.md`.
- `/Users/gdc/deadreckon/crates/deadreckon-core/src/{workspace_capture,completion,promotion}.rs`; `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/{turn_loop,semantic_judge}.rs`.
- Git ignore semantics, Bazel sandboxing and Nix derivation outputs, linked in the rider as exemplars.

**Posture.** Stable track. Add controller-owned result-projection files and additive proof fields only; no `PipelineState`, `Job`, launch-plan or authority schema change. Preserve old receipts and active Jobs. No output-name registry, live provider, push, release or edits outside `/Users/gdc/deadreckon`.

**Settled behavior.**

- Admission capture remains immutable authority for original tracked paths and provenance. A final project-local `.gitignore`/`.ignore` is an untrusted proposal; late global or `.git/info/exclude` rules are never promotion authority.
- Original tracked paths always remain in the candidate. Final project-local ignores may omit provider-created untracked output. Negation rules and explicit non-ignored paths preserve intended generated deliverables.
- After provider quiescence, the controller writes a projection policy and omission manifest, materializes one candidate outside provider authority, and seals candidate tree hash `H` plus projection hash `P`.
- Deterministic acceptance runs in a disposable materialization of `H`; its writes never join the candidate. The marker binds the projection. The independent semantic judge sees the clean candidate and omission manifest. The receipt binds `H`, `P`, marker and judgment.
- Promotion copies only the sealed projection and rehashes the published tree. Any mutation, projection drift or proof/tree mismatch refuses.
- Unclassified ambiguity fails visibly as review-required evidence; it is not converted into repeated generic retries.

**Phases.** Eleven (P1–P11) in the rider. Each begins with named depth tests observed red, then implementation, focused green validation and a conventional local commit. Milestones run the release verification chain. P11 updates AS-BUILT, CHANGELOG and the operator checklist.

**Verification.**

- Greenfield Next.js and unknown-framework fixtures with no ignore at admission add final ignores, generate churning output, and promote without built-in output names.
- A required new source hidden by a late ignore cannot receive a verified receipt; an originally tracked path cannot be hidden.
- Two identical `dist/` fixtures prove intent: ignored output is omitted, non-ignored requested output is shipped.
- Gate-created random output is absent from semantic evidence, receipt and promotion; candidate mutation at every boundary fails closed.
- Existing strict gate, semantic, receipt, promotion, worktree, Graph/Campaign and legacy compatibility suites remain green.

**Stop when** every rider depth test and release-level verification pass, AS-BUILT and CHANGELOG are honest, changes are committed locally, and the operator has an exact manual acceptance script.
