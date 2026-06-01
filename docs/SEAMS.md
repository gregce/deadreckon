# DeadReckon Seams

Seams let you replace four governance workers with JSON-over-stdio subprocesses:
policy, model catalog, hooks, and event sink. They are configured in
`config.toml` under `[seams]`. The acceptance gate is deliberately not a seam.

## Example Kit

The runnable conformance examples live here:

- `examples/seams/config.toml`
- `examples/seams/fixtures/policy-allow.json`
- `examples/seams/fixtures/policy-deny.json`
- `examples/seams/fixtures/catalog-request.json`
- `examples/seams/fixtures/hook-event.json`
- `examples/seams/fixtures/event-sink-event.json`
- `examples/seams/workers/policy-allow.sh`
- `examples/seams/workers/policy-deny.sh`
- `examples/seams/workers/catalog-minimal.sh`
- `examples/seams/workers/hooks-jsonl.sh`
- `examples/seams/workers/event-sink-jsonl.sh`

Run examples from the repository root:

```sh
deadreckon seams validate policy --config examples/seams/config.toml --fixture examples/seams/fixtures/policy-allow.json --sandbox none
deadreckon seams validate catalog --config examples/seams/config.toml --fixture examples/seams/fixtures/catalog-request.json --sandbox none
deadreckon seams validate hooks --config examples/seams/config.toml --fixture examples/seams/fixtures/hook-event.json --sandbox none
deadreckon seams validate event-sink --config examples/seams/config.toml --fixture examples/seams/fixtures/event-sink-event.json --sandbox none
```

For live runs, prefer absolute worker paths or commands on `PATH`; a run
workspace may not have the repository root as its working directory.

## Config

```toml
[seams.policy]
command = ["sh", "examples/seams/workers/policy-allow.sh"]
timeout_ms = 1000

[seams.catalog]
command = ["sh", "examples/seams/workers/catalog-minimal.sh"]
timeout_ms = 1000

[seams.hooks]
command = ["sh", "examples/seams/workers/hooks-jsonl.sh"]
timeout_ms = 1000

[seams.event_sink]
command = ["sh", "examples/seams/workers/event-sink-jsonl.sh"]
timeout_ms = 1000
```

Allowed kinds are `policy`, `catalog`, `hooks`, and `event_sink`. Unknown kinds
are config errors. `[seams.gate]` is rejected because the gate is the local
trust root.

## Worker Protocol

Each worker receives one JSON object on stdin and should write one JSON object
to stdout before exiting. Workers run without network access through the
configured sandbox backend. `timeout_ms = 0` is invalid.

Policy receives tool-decision context such as `function_id`, `command`, and
`working_dir`. It must return:

```json
{"decision":"allow"}
```

or:

```json
{"decision":"deny","reason":"explain the denial"}
```

Policy is fail-closed. Timeout, non-zero exit, invalid JSON, or any unknown
decision blocks the action.

Catalog receives a catalog request object and returns model metadata:

```json
{"models":[{"id":"local-scripted-smoke","context_window":4000}]}
```

Catalog is fail-open. Invalid output is visible in validation, but the runtime
falls back to the built-in catalog.

Hooks observe tool lifecycle events. Event sink observes run events. Both are
fail-safe: failure is visible, but neither can control acceptance or tool
decisions.

## Validation

Use `deadreckon seams validate` before putting a worker in a run:

```sh
deadreckon seams validate policy --config examples/seams/config.toml --json --sandbox none
```

Plain output includes kind, command, fixture, sandbox, outcome, fail policy, and
`try:` lines on failure. JSON output is intended for focused tests and scripts.

`event-sink` is the CLI spelling; `event_sink` is the TOML key.

## Sandbox and Gate Boundary

Seam subprocesses inherit the seam sandbox, run without network, and get only
their JSON request through stdin plus a minimal environment. The runtime denies
seam access to the run's `gate/` and `proofs/` subtrees. The conformance
validator uses the same dispatch path.

`dr-gate` remains local and deterministic. A seam can observe or narrow
behavior, but it cannot issue, redirect, or sign an acceptance marker.

## Disabling Seams

Use `--no-seams` to force all built-in behavior for one launch:

```sh
deadreckon run "goal" --no-seams
deadreckon start "goal" --no-seams
```

`deadreckon doctor` reports configured seam commands and their fail policies.
