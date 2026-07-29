# Watchkeeper dogfood

This directory is an operator-triggered trial kit for durable DeadReckon jobs.
Nothing here starts paid providers during tests or installation.

The harness exercises the ordinary public journey:

1. `deadreckon start`
2. repeated `deadreckon status`
3. validation of `$DEADRECKON_HOME/jobs/<id>/receipt.json`
4. `deadreckon finish`

Use disposable clones. `finish` can land verified work into the selected
checkout.

## Prerequisites

- a built `deadreckon` binary;
- Python 3;
- two disposable repository checkouts;
- two configured provider routes;
- a compiled and approved definition of done in each checkout;
- a sandbox backend capable of producing a contained receipt.

Map the repository and provider slots declared in `matrix.json`:

```sh
export DEADRECKON_DOGFOOD_REPO_A=/path/to/disposable/deadreckon
export DEADRECKON_DOGFOOD_REPO_B=/path/to/disposable/fixture-app
export DEADRECKON_DOGFOOD_PROVIDER_A=cli:codex
export DEADRECKON_DOGFOOD_PROVIDER_B=cli:claude-code
export DEADRECKON_HOME=/path/to/dogfood-state
export DEADRECKON_BIN=/path/to/deadreckon
```

Inspect a task, then explicitly enable execution:

```sh
python3 examples/watchkeeper-dogfood/collect-metrics.py --help
DEADRECKON_DOGFOOD_EXECUTE=1 \
  examples/watchkeeper-dogfood/run.sh dr-self-01
```

The harness writes the public command outputs and a final `job-view.json` under
`artifacts/`. It stops without calling `finish` when the combined receipt is
missing, uncontained, for a different job, or not a verified two-key result.

Generate metrics from the persisted observations:

```sh
python3 examples/watchkeeper-dogfood/collect-metrics.py \
  --home "$DEADRECKON_HOME" \
  --observations examples/watchkeeper-dogfood/artifacts \
  --output examples/watchkeeper-dogfood/artifacts/metrics.json
```

The collector reads the public `status --json` JobView, append-only job events,
completion receipt, and semantic judgment. It never reads narrative or
implementation-note prose. Human-only measurements remain `null` until an
operator writes a structured `human-review.json` based on
`human-review.template.json`.

All 24 matrix entries started as `not_run`. The sanitized
`trial-results.json` now records two operator attempts:

- `dr-self-02` stopped `retry_exhausted` / `attempt_limit` after three
  identical provider-authentication failures;
- `fixture-01` completed worker and deterministic checks, then stopped
  `needs_review` / `semantic_unavailable` because the judge result had an
  unsupported provider-event envelope.

Neither attempt produced a receipt or reached `finish`; 22 tasks remain
`not_run`. The fixture attempt also proved detached continuation after the
original harness process exited while parsing mixed human and JSON output.
Raw provider exchanges, credentials and absolute paths are not stored in the
checked-in result.

## Run the credential-free adversarial matrix

The adversarial runner executes the repository's focused process, recovery,
sandbox, receipt and delivery tests. It does not read provider credentials:

```sh
python3 examples/watchkeeper-dogfood/adversarial.py \
  --output examples/watchkeeper-dogfood/credential-free-results.json
```

The JSON output records each command, duration, exit status, matched test line,
and stdout/stderr digest. It labels proof by scope. The runner first proves
that `sandbox-exec` can apply a profile; nested environments that deny that
operation report the host-sandbox trials as `unproven`. The network test
proves a local server is reachable without containment and unreachable through
DeadReckon's Seatbelt profile. A local Seatbelt pass does not claim a live
provider, another sandbox backend, an active user service, or a reboot.

The following remain `unproven` in that file until an operator performs them:

- worker death during an approved live provider run;
- supervisor death during an approved live provider run;
- host network loss during a live provider call;
- a real machine reboot with the user service active;
- a cross-provider hostile gate trial from the 24-task matrix.
