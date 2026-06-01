# deadreckon - Seam Conformance Kit Rider (make the contract executable)

Goal pointer: `/Users/gdc/deadreckon/docs/goals/2026-05-31-1857-deadreckon-seam-conformance-kit-goal.md`

This rider turns the already-built composable seam design into something a user can run, copy, validate, and debug. The previous turn added the primitive and wiring. This turn should not redesign it. It should make the implicit protocol executable.

## Posture and Invariants

- Production-release track. The outcome should feel like a supported extension point, not an experiment.
- Keep the existing seam primitive: per-call subprocess worker, JSON stdin/stdout, timeout, cwd/env controls, sandbox preference, and fixed fail policies.
- The gate is not a seam. Acceptance-marker issuance, proof paths, nonce handling, and tamper evidence must remain local and non-swappable.
- Do not change durable `PipelineState`, `Plan`, `AcceptanceMarker`, `ProviderEntry`, or ledger schemas.
- No worker registry, daemon, WebSocket bus, cloud marketplace, human-approval seam, or version-negotiation framework.
- No network dependency in examples or tests.
- No `git push`.

## Existing Surface To Read

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` section 39 for the current seam contract.
- `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/seam.rs` for `SeamKind`, `SeamCommand`, config parsing, sandbox selection, audit behavior, and `dispatch_seam`.
- `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/turn_loop.rs` for policy, catalog, hooks, event-sink integration, and `--no-seams`.
- `/Users/gdc/deadreckon/crates/deadreckon/src/commands/doctor.rs` for existing diagnostic style.
- `/Users/gdc/deadreckon/crates/deadreckon/src/cli.rs` for command structure.
- `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` for deferred decisions.

## Desired Artifact Shape

Create an examples tree:

- `/Users/gdc/deadreckon/examples/seams/README.md`
- `/Users/gdc/deadreckon/examples/seams/config.toml`
- `/Users/gdc/deadreckon/examples/seams/fixtures/policy-allow.json`
- `/Users/gdc/deadreckon/examples/seams/fixtures/policy-deny.json`
- `/Users/gdc/deadreckon/examples/seams/fixtures/catalog-request.json`
- `/Users/gdc/deadreckon/examples/seams/fixtures/hook-event.json`
- `/Users/gdc/deadreckon/examples/seams/fixtures/event-sink-event.json`
- `/Users/gdc/deadreckon/examples/seams/workers/policy-allow.sh`
- `/Users/gdc/deadreckon/examples/seams/workers/policy-deny.sh`
- `/Users/gdc/deadreckon/examples/seams/workers/catalog-minimal.sh`
- `/Users/gdc/deadreckon/examples/seams/workers/hooks-jsonl.sh`
- `/Users/gdc/deadreckon/examples/seams/workers/event-sink-jsonl.sh`

Use POSIX `sh` for worker examples unless the repo already has a stronger local pattern. Keep scripts short, deterministic, and self-contained. If executable mode is hard to preserve in patch flow, add tests/docs that call them through `sh`.

Add user docs:

- Preferred: `/Users/gdc/deadreckon/docs/SEAMS.md`
- Also update `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` section 39 with a compact pointer to the conformance kit.
- Update `/Users/gdc/deadreckon/CHANGELOG.md` if that file exists and is the repo's current changelog surface.

## Validation Surface

Preferred CLI:

```text
deadreckon seams validate <kind> --config <path> [--fixture <path>] [--json] [--sandbox <backend>]
```

Where `<kind>` is one of:

- `policy`
- `catalog`
- `hooks`
- `event-sink`

Use `--config` to read the configured worker from `[seams.<kind>]`. This keeps argv parsing simple and validates the same config shape users will actually run. If the existing CLI shape makes a top-level `seams` command too invasive, implement the same checks under `deadreckon doctor --seams` or a similarly local diagnostic surface, but keep the behavior and tests.

Plain output should be terse and actionable:

- resolved kind and command basename
- fixture path
- sandbox backend actually selected or disabled
- pass/fail result
- fail policy for the kind
- one or two `try:` lines on failure

JSON output should be stable enough for tests:

```json
{
  "kind": "policy",
  "command": "policy-allow.sh",
  "fixture": "examples/seams/fixtures/policy-allow.json",
  "sandbox": "sandbox-exec",
  "outcome": "passed",
  "fail_policy": "closed",
  "try": []
}
```

Do not invent a durable schema for this output unless the codebase already treats CLI JSON as a public contract. A small local struct for serialization is enough.

## Protocol Expectations

Policy:

- Request fixture should represent the same request shape runtime policy dispatch uses.
- Valid allow response: `{"decision":"allow"}`.
- Valid deny response: `{"decision":"deny","reason":"..."}`.
- Timeout, non-zero exit, malformed JSON, or unknown decision must be reported as fail-closed.

Catalog:

- Request fixture should match runtime catalog resolution enough to catch drift.
- Valid response should include a minimal model list, including model id and context-window metadata when that is what runtime consumes.
- Timeout, non-zero exit, or malformed JSON must be reported as fail-open.

Hooks:

- Hook worker output is observe-only. It may return valid JSON or no meaningful result, depending on current primitive behavior.
- Timeout/non-zero/malformed output must be reported as non-fatal.

Event sink:

- Event-sink worker output is observe-only.
- Timeout/non-zero/malformed output must be reported as non-fatal.

Gate/proofs:

- No validation path should pass gate proof paths, acceptance marker internals, nonce material, or writable proof directories to a seam worker.
- Add an adversarial example or fixture test that tries to observe or mutate those paths and proves it cannot.

## Phases

### P1 - Fixture Contract Before Code

Add fixture JSON and a sample config. Write depth tests first:

- `seam_example_fixtures_are_valid_json`
- `example_config_uses_known_seam_kinds`
- `example_config_paths_exist`

The tests should fail before the files exist and pass once the examples are added.

### P2 - Example Workers Round Trip

Add the five worker scripts. Use existing `dispatch_seam` where possible in tests so examples exercise the same primitive as production.

Depth tests:

- policy allow worker returns allow through `dispatch_seam`
- policy deny worker returns deny through `dispatch_seam`
- catalog worker returns a minimal catalog override through `dispatch_seam`
- hooks and event-sink workers accept observe-only events without affecting control flow

### P3 - CLI Skeleton

Add the validation command in the smallest place that matches current CLI organization.

Depth tests:

- help output includes `seams validate` or the chosen diagnostic command
- invalid kind is rejected by clap or by a clear validation error
- missing config prints a `try:` line and exits non-zero

### P4 - Policy Validation

Implement policy validation against fixture input.

Depth tests:

- allow worker passes
- deny worker is recognized as a valid denial, not as a malformed worker
- malformed policy response exits non-zero and reports fail-closed
- timeout/non-zero reports fail-closed

### P5 - Catalog Validation

Implement catalog validation.

Depth tests:

- catalog example passes and includes expected model/context metadata
- malformed catalog response exits non-zero and reports fail-open
- timeout/non-zero reports fail-open

### P6 - Hooks and Event Sink Validation

Implement observe-only validation for hooks and event sink.

Depth tests:

- hook example passes
- event-sink example passes
- non-zero hook or sink worker is reported as non-fatal but still visible
- JSON output preserves kind/outcome/fail policy

### P7 - Gate Boundary Proof

Add an adversarial test around validation and examples.

Depth tests:

- seam validation does not expose acceptance-marker paths
- seam validation does not expose nonce material
- seam validation cannot create or mutate proof artifacts through configured cwd/env

Use temp directories and local fixtures. Do not weaken sandbox behavior to make this easy.

### P8 - User Docs

Write `docs/SEAMS.md` or the local equivalent. It should document:

- what each seam is for
- config shape
- request/response examples
- fail policies
- sandbox expectations and platform caveats
- how to run validation
- how `--no-seams` disables all seams
- why the gate is not a seam

Depth test:

- docs reference only example paths that exist
- command snippets use real fixture/config paths

### P9 - Runtime Smoke With Example Config

Add focused integration coverage using the example config.

Depth tests:

- deny policy blocks before provider work would proceed
- `--no-seams` restores built-in behavior for the same config
- hook/event-sink examples receive events without controlling acceptance

Keep this smoke local and deterministic. Avoid real provider/network calls.

### P10 - Output Polish

Make failures useful without making output chatty.

Depth tests:

- plain output includes kind, command, fixture, sandbox, outcome, and `try:` lines
- JSON output is parseable and contains no absolute temp paths unless unavoidable
- no configured seam gives a clear skip or failure state, whichever is consistent with the chosen UX

### P11 - Docs, Changelog, Candidate Ledger

Update:

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`
- `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` if any scope is intentionally deferred
- `/Users/gdc/deadreckon/CHANGELOG.md` if present

No depth test is required for prose, but run `git diff --check`.

## Verification Matrix

Run focused checks before committing:

- `cargo fmt --check`
- `cargo test -p deadreckon-runtime seam`
- CLI package tests for the validation command
- `git diff --check`

If package names differ, choose the nearest focused Cargo invocations rather than broad, slow suites first. Run broader tests only if touched code crosses shared command/runtime boundaries.

Manual sanity:

- every example worker validates from `/Users/gdc/deadreckon/examples/seams/config.toml`
- deny policy is a valid worker result, not a validation failure
- malformed catalog result reports fail-open
- hook/event sink failure is visible but non-fatal
- `--no-seams` remains documented and tested

## Commit

Commit locally once verification passes. Suggested message:

```text
Add seam conformance kit
```

Do not push.
