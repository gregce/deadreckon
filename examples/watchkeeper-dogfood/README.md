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

All 24 matrix entries start as `not_run`. A successful local test of the
scripts does not imply that any provider trial ran.
