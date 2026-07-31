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
export DEADRECKON_DOGFOOD_ARTIFACTS=/path/to/dogfood-observations
```

Inspect a task, then explicitly enable execution:

```sh
python3 examples/watchkeeper-dogfood/collect-metrics.py --help
DEADRECKON_DOGFOOD_EXECUTE=1 \
  examples/watchkeeper-dogfood/run.sh dr-self-01
```

The harness writes the public command outputs and a final `job-view.json` under
`$DEADRECKON_DOGFOOD_ARTIFACTS`. Keep that directory outside either disposable
source checkout. It stops without calling `finish` when the public report
cannot validate a contained two-key receipt for the Job.

Generate metrics from the persisted observations:

```sh
python3 examples/watchkeeper-dogfood/collect-metrics.py \
  --home "$DEADRECKON_HOME" \
  --observations "$DEADRECKON_DOGFOOD_ARTIFACTS" \
  --output "$DEADRECKON_DOGFOOD_ARTIFACTS/metrics.json"
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

The checked-in result currently records 12 passing credential-free proof
groups and no failures. The added proof groups directly cover supported
creation routes entering one Job journey, one- and two-round Graph semantic
parent repair, one-round Campaign semantic parent repair, candidate-ready
recovery, and all seven repair-receipt lineage attacks. They also prove that
Job-owned child Runs cannot bypass their parent lifecycle and that hostile
read-only planning cannot write to the operator workspace while a benign
planner remains usable under an operational sandbox. An opt-in live Docker
trial additionally proves the real container's key, environment, network and
control-path boundary when `rust:1` is already cached; it never pulls an image
implicitly. Nine public live or host-specific claims remain explicitly
unproven.

The JSON output records each command, duration, exit status, matched test line,
and stdout/stderr digest. It labels proof by scope. The runner first proves
that `sandbox-exec` can apply a profile; nested environments that deny that
operation report the host-sandbox trials as `unproven`. The network test
proves a local server is reachable without containment and unreachable through
DeadReckon's Seatbelt profile. The Docker control-boundary pass does not claim
that a macOS Mach-O `dr-gate` can execute inside a Linux container. Neither
host-backed pass claims a live provider, an active user service or a reboot.

The following remain `unproven` in that file until an operator performs them:

- worker death during an approved live provider run;
- supervisor death during an approved live provider run;
- host network loss during a live provider call;
- a real machine reboot with the user service active;
- a cross-provider hostile gate trial from the 24-task matrix;
- a naturally occurring live semantic `revise` followed by Graph or Campaign
  parent repair;
- live Campaign interruption and recovery without duplicate sub-plan or repair
  work;
- the equivalent protected gate boundary on Linux/bubblewrap;
- a public strict Docker Job running a platform-compatible `dr-gate`; the
  common live Docker control boundary is covered separately.

## Record an operator-run live fault trial

`live-trial.py` never starts a provider, performs the fault, changes
networking, controls a service, reboots or calls `finish`. The operator still
performs the reviewed intervention. In trusted mode the recorder asks the
private `dr-capture` helper for exact canonical evidence; it does not accept
operator-selected evidence files as proof.

Use a canonical `dr-capture` and sibling `deadreckon` pair outside the Job
source, working, run, merge and repair roots. Pass-capable prepare fails closed
for in-place or uncontained Jobs. Name every provider by its manifest role;
worker and independent-judge routes must be different.

```sh
export WK_TRIAL_ID=live_provider_supervisor_restart
export WK_LIVE_TRIAL="$DEADRECKON_DOGFOOD_ARTIFACTS/live/$WK_TRIAL_ID-01"
export WK_JOB_ID=the-full-approved-job-id
export DR_CAPTURE_BIN=/protected/deadreckon/bin/dr-capture
export DEADRECKON_BIN=/protected/deadreckon/bin/deadreckon

python3 examples/watchkeeper-dogfood/live-trial.py prepare \
  "$WK_TRIAL_ID" \
  --trial-dir "$WK_LIVE_TRIAL" \
  --revision "$(git rev-parse HEAD)" \
  --capture-helper "$DR_CAPTURE_BIN" \
  --deadreckon-binary "$DEADRECKON_BIN" \
  --job-id "$WK_JOB_ID" \
  --backend sandbox-exec \
  --provider-route worker=cli:codex \
  --provider-route independent_judge=cli:claude-code
python3 -m json.tool "$WK_LIVE_TRIAL/replay.json"
```

Prepare is retry-safe: the same inputs reuse the same capture session and
protected binding after an interruption. Conflicting inputs are refused.
`replay.json` records the exact canonical subjects, trial-specific
intervention source and cleanup source. Capture the declared `before`
subjects:

```sh
python3 examples/watchkeeper-dogfood/live-trial.py observe \
  --trial-dir "$WK_LIVE_TRIAL" \
  --canonical job-view-before \
  --canonical events-before \
  --canonical lease-before \
  --canonical supervised-child-before
```

Perform the reviewed intervention outside the recorder, then record that
boundary separately. The detail file is operator context only; `dr-capture`
independently reads the trial-specific Job, Campaign or sandbox observation:

```sh
python3 examples/watchkeeper-dogfood/live-trial.py observe \
  --trial-dir "$WK_LIVE_TRIAL" \
  --intervention-status performed \
  --intervention-detail-file /path/to/operator-intervention-evidence
```

Only then record the declared `after` evidence:

```sh
python3 examples/watchkeeper-dogfood/live-trial.py observe \
  --trial-dir "$WK_LIVE_TRIAL" \
  --canonical job-view-after \
  --canonical events-after \
  --canonical lease-after \
  --canonical supervised-child-after \
  --canonical job-report
```

Complete the manifest cleanup, then finalize. Finalization writes two
immutable files. `result.evaluation.json` is the pre-seal evaluation with no
receipt claim. `result.json` is the minimal publication envelope containing
the exact evaluation, its digest, the protected receipt digest and the
receipt's HMAC publication proof.

```sh
python3 examples/watchkeeper-dogfood/live-trial.py cleanup \
  --trial-dir "$WK_LIVE_TRIAL" \
  --status completed \
  --detail-file /path/to/cleanup-evidence
python3 examples/watchkeeper-dogfood/live-trial.py finalize \
  --trial-dir "$WK_LIVE_TRIAL" \
  --evaluation-output "$WK_LIVE_TRIAL/result.evaluation.json" \
  --output "$WK_LIVE_TRIAL/result.json"
python3 -m json.tool "$WK_LIVE_TRIAL/result.json"
```

The helper re-runs the binding-hashed recorder over a bundle reconstructed
only from protected exact evidence and authenticated lifecycle history.
Submitted status, oracle assertions and evidence metadata must byte-match
that deterministic result. It then verifies the published envelope against
the protected receipt:

```sh
"$DR_CAPTURE_BIN" verify \
  --job-id "$WK_JOB_ID" \
  --session-id "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["session_id"])' "$WK_LIVE_TRIAL/trial-state.json")" \
  --result "$WK_LIVE_TRIAL/result.evaluation.json" \
  --envelope "$WK_LIVE_TRIAL/result.json"
```

This verification is the authority for `verified`; JSON shape validation
alone is not. The envelope remains sanitized: it contains no raw evidence,
credentials, full Job IDs, notes, helper paths or capture paths.

Omit the trusted prepare arguments to retain the manual compatibility path.
That mode accepts `--capture NAME=/path/to/file`, labels the result
`operator_attested`, and can never produce `status: passed`.
