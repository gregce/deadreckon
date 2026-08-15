# Workspace capture and generated output

DeadReckon snapshots source and other recoverable workspace state. It does not
copy disposable build output such as SwiftPM `.build/` into every snapshot.
This document explains the trust boundary, the evidence left by a run, and the
operator proof for that behaviour.

## What is frozen before work starts

Each new run writes `workspace-capture-policy.json` below its run root before a
provider may change the workspace. The policy records:

- the original `.gitignore`, `.ignore`, `.git/info/exclude`, and configured
  global Git excludes;
- the exact tracked paths returned by `git ls-files -z --cached`, including
  non-UTF-8 paths on Unix;
- the original Git head and index digest;
- output directories discovered from Cargo metadata, SwiftPM, Bazel, CMake,
  and Gradle; and
- the capture budgets used by the run.

Tracked files remain recoverable even when an ignore or output rule matches
them. A provider changing an ignore file does not change this admission policy.
This prevents an accidental or hostile ignore rewrite from hiding an original
deliverable or changing source hydration, snapshots, checkpoints or provenance.

Older persisted policies are upgraded from their already-frozen tracked-path,
HEAD, and index fields without consulting live Git. If an active run has no
policy after provider work may have begun, DeadReckon fails closed instead of
manufacturing trust from the current workspace.

## What is sealed when work finishes

Strict Jobs have a second, narrower boundary for the operator-visible result.
After the provider stops and the final documents exist, DeadReckon treats the
final project-local `.gitignore` and `.ignore` files as an untrusted proposal
for what should not ship. It never consults late host-global excludes or
`.git/info/exclude`, and every admission-tracked path still wins over a late
ignore rule.

DeadReckon materialises the proposed result outside the provider workspace and
records `result-projection/policy.json`, `manifest.json` and `candidate/` under
the run root. The manifest binds the admission policy, projection policy,
omissions, file count, byte count and exact tree digest. This mechanism has no
list of framework directory names: `.next`, `.venv`, a future tool's cache, or
any other provider-created output is omitted only when the project's final
local ignore rules say so. Conversely, an unignored `dist/` remains part of the
result when it is intentionally deliverable.

That one sealed candidate is the only input to later authority:

1. trusted Git staging uses its exact policy and refuses workspace drift;
2. the deterministic gate runs on a disposable copy and cannot write back;
3. the semantic judge sees the sealed candidate and omission manifest;
4. the signed marker and receipt bind the projection and result-tree digest;
5. promotion rematerialises the candidate and revalidates the same receipt.

The complete definition of done is unchanged. Ignore rules can propose a file
set; they cannot waive an acceptance check, semantic review, containment proof,
receipt validation or exact promotion. An unsafe or oversized projection ends
as `NEEDS_REVIEW` rather than silently dropping data or consuming retries.

## How capture remains bounded

Source hydration, turn snapshots, flight checkpoints, working-file indexes,
deliverable indexes, and workspace-guard indexes use the same ignore-aware
walker and policy. The default per-capture limits are:

| Limit | Default |
| --- | ---: |
| Files | 100,000 |
| Total bytes | 512 MiB |
| One file | 128 MiB |
| Traversal time | 10 seconds |

Known output roots are pruned before admission-time snapshot and recovery
traversal. The sealed result projection does not use those names. An unrecognised subtree is
treated as suspicious generated output when it has at least 2,000 files and
128 MiB, with at least 60% generated-looking files. DeadReckon stores a bounded
summary containing its path, file count, byte count, and digest instead of its
contents.

Every omission is explicit. A partial capture is materialised for inspection,
but DeadReckon refuses to use it for exact restore, rewind, source hydration, or
trusted Git staging. This makes a missed classification a bounded, visible
refusal rather than a hang or silent data loss.

## Evidence and disk use

A run leaves the following records under its run root:

- `workspace-capture-policy.json`: the frozen inputs and limits;
- `source-hydration-manifest.json`: the initial source-copy result, when used;
- `snapshot-manifests/turn-N.json`: inclusion, omission, generated-output, Git,
  and materialisation statistics for each turn snapshot; the same manifest is
  written inside the staged snapshot before that snapshot is atomically made
  visible;
- `checkpoints/<id>/manifest.json`: the equivalent record for flight
  checkpoints; and
- `workspace-blobs/sha256/`: run-scoped immutable whole-file blobs.
- `result-projection/{policy.json,manifest.json,candidate/}`: the exact strict
  Job result selected after provider quiescence.

Snapshots and checkpoints hard-link to a matching content blob when the
filesystem supports it and fall back to a normal copy otherwise. The manifest
reports new blobs, reused blobs, hard links, and fallbacks. Mutable working
directories are always ordinary files; they are never hard-linked to evidence
blobs.

## Operator acceptance

Run the regression proof from the repository root:

```bash
cd /Users/gdc/deadreckon
cargo test -p deadreckon-core workspace_capture::tests --lib
cargo test -p deadreckon-core artifacts::tests --lib
cargo test -p deadreckon-core flight::tests --lib
cargo test -p deadreckon-core result_projection::tests --lib
cargo test -p deadreckon-core \
  strict_promotion_publishes_the_sealed_projection --lib
cargo test -p deadreckon-core signed_result_projection --lib
cargo test -p deadreckon-runtime semantic_judge::tests --lib
cargo test -p deadreckon-runtime \
  turn_commit_archives_private_artifacts_and_rewrites_provider_commits --lib
```

Observable success is four `test result: ok` summaries. These tests prove that:

- a SwiftPM `.build/` tree is omitted and summarised;
- hard file and byte limits return a reported partial capture;
- tracked files override ignores and generated-output classification;
- changing `.gitignore` during a turn cannot admit a previously ignored cache;
- partial snapshots and checkpoints cannot be restored as exact state; and
- a second snapshot reuses the first snapshot's content blobs;
- agent-staged build output cannot change the frozen tracked-file authority;
  and
- promotion materialises the same bounded, trusted capture it records and
  rejects later payload tampering.
- unknown ignored output is excluded without a framework registry while an
  unignored generated artifact remains deliverable;
- gate scratch writes cannot mutate the sealed result;
- marker/receipt tampering invalidates the proof; and
- strict promotion publishes and revalidates the exact sealed tree.

For a live run, inspect the capture evidence after the first turn:

```bash
deadreckon show <run-id>
jq '{frozen_at, output_roots, budgets, warnings}' \
  <run-root>/workspace-capture-policy.json
jq '{partial, included_files, included_bytes, generated_outputs, omissions, materialization}' \
  <run-root>/snapshot-manifests/turn-1.json
du -sh <run-root>/snapshots <run-root>/workspace-blobs
```

Replace `<run-id>` and `<run-root>` with the values printed by `deadreckon
show`. For a SwiftPM workspace, `output_roots` should name `.build` (or the
configured Swift output), `generated_outputs` should summarise it, and the
snapshot directory should not contain `.build` contents. On a filesystem with
hard-link support, later snapshot manifests should report `reused_blobs` and
`hardlinks` greater than zero.

This proof does not establish recovery after an operating-system restart or
semantic goal completion. Those are separate supervisor and completion-gate
acceptance boundaries.
