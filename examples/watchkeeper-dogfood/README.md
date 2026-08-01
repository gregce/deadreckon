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

Plan the whole matrix first. This command is read-only: it does not start a
provider even if `DEADRECKON_DOGFOOD_EXECUTE=1` is already present in the
environment.

```sh
python3 examples/watchkeeper-dogfood/batch.py
```

After reviewing that plan, execution requires both the command flag and the
existing environment gate:

```sh
DEADRECKON_DOGFOOD_EXECUTE=1 \
  python3 examples/watchkeeper-dogfood/batch.py --execute
```

The batch stops on the first non-zero task result. Run the planning command
again to inspect the resume posture. It skips a task only when its terminal `JobView` and
`operator-run.json` agree on the exact matrix digest, task ID, Job ID,
repository slot and provider slot. Partial or malformed artifacts are never
treated as completed. Any non-empty task directory without exactly one valid
terminal observation blocks execution until its recorded Job is inspected and
the artifacts are archived or repaired. This fail-closed boundary prevents a
batch killed after `start` from launching a second Job on restart. Keep
`matrix.json` frozen once execution begins because any byte change
intentionally invalidates prior observations. The batch passes the reviewed
matrix path, artifact root, and digest into every runner invocation; the runner
refuses if the matrix bytes change after planning. To run one reviewed task directly, the compatible
single-task command remains
`DEADRECKON_DOGFOOD_EXECUTE=1 ./examples/watchkeeper-dogfood/run.sh TASK_ID`.
The direct runner atomically claims a new task artifact directory before
calling `deadreckon start`. Even an existing empty directory is treated as an
existing attempt and must be inspected and archived rather than reused.

The harness writes the public command outputs and a final `job-view.json` under
`$DEADRECKON_DOGFOOD_ARTIFACTS`. Keep that directory outside either disposable
source checkout. It stops without calling `finish` when the public report
cannot validate a contained two-key receipt for the Job.

Generate metrics from the persisted observations:

```sh
python3 examples/watchkeeper-dogfood/collect-metrics.py \
  --home "$DEADRECKON_HOME" \
  --observations "$DEADRECKON_DOGFOOD_ARTIFACTS" \
  --matrix examples/watchkeeper-dogfood/matrix.json \
  --output "$DEADRECKON_DOGFOOD_ARTIFACTS/metrics.json"
```

The collector reads the public `status --json` JobView, append-only job events,
completion receipt, and semantic judgment. It never reads narrative or
implementation-note prose. Human-only measurements remain `null` until an
operator writes a structured `human-review.json` based on
`human-review.template.json`. Copying the untouched template is not a review:
the collector requires the exact fields, matching Job ID, timezone-aware
timestamp, named reviewer, booleans, and finite nonnegative measurements.

The metrics artifact binds the raw matrix bytes by SHA-256 and lists every
task's repository and provider slots. It reports missing, attempted, completed,
verified, and reviewed task IDs separately. Execution is `complete` only when
all 20–30 matrix tasks have one unambiguous terminal observation spanning at
least two repository slots and two provider slots. Assessment is `ready`, and
`campaign_completion.claim_allowed` becomes true, only when every completed
task also has a valid human review. This keeps execution completeness separate
from the evidence needed to make the product claim. Missing factual artifacts,
invalid event rows, and symlinked or non-file event histories also keep the
assessment incomplete rather than being interpreted as zero activity. Copied
reports cannot be symlinks, and semantic spend is read only from the Run
identity's exact path under `$DEADRECKON_HOME/runstate`.

The batch, direct runner, and collector share one matrix parser. Task and slot
IDs must be safe path components, environment-variable references must be safe
names, spend must be finite and nonnegative, and an optional `task_count` must
equal the number of task rows.

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
sandbox, receipt and delivery tests. It does not read provider credentials.
Generate checked evidence from a clean detached worktree and write the first
result outside that worktree:

```sh
WK_SOURCE_REV=$(git rev-parse HEAD)
WK_PROOF_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/deadreckon-watchkeeper.XXXXXX")
WK_SOURCE_WORKTREE="$WK_PROOF_ROOT/source"
WK_PROOF_RESULT="$WK_PROOF_ROOT/credential-free-results.json"

git worktree add --detach "$WK_SOURCE_WORKTREE" "$WK_SOURCE_REV"
env PYTHONDONTWRITEBYTECODE=1 CARGO_TARGET_DIR="$WK_PROOF_ROOT/target" \
  python3 "$WK_SOURCE_WORKTREE/examples/watchkeeper-dogfood/adversarial.py" \
    --repo "$WK_SOURCE_WORKTREE" \
    --output "$WK_PROOF_RESULT"
test -z "$(git -C "$WK_SOURCE_WORKTREE" status --porcelain --untracked-files=all)"
```

The public strict Docker group runs automatically only when preflight finds a
running daemon, an already-cached `rust:1` image for `arm64/linux`, and a static
Linux arm64 evaluator beside Cargo's debug `deadreckon` binary. The runner
never pulls or builds either prerequisite. To make that group pass, install
`cargo-zigbuild` 0.23.0 and Zig 0.14.1, then prepare the evaluator outside the
clean source worktree before running the command above:

```sh
rustup target add aarch64-unknown-linux-musl
(
  cd "$WK_SOURCE_WORKTREE"
  CARGO_TARGET_DIR="$WK_PROOF_ROOT/target" cargo zigbuild \
    --release --locked --no-default-features \
    -p deadreckon --bin dr-gate \
    --target aarch64-unknown-linux-musl
)
mkdir -p "$WK_PROOF_ROOT/target/debug"
install -m 0755 \
  "$WK_PROOF_ROOT/target/aarch64-unknown-linux-musl/release/dr-gate" \
  "$WK_PROOF_ROOT/target/debug/dr-gate-evaluator-aarch64-unknown-linux-musl"
node "$WK_SOURCE_WORKTREE/release/evaluator-sidecars.mjs" verify-sidecars \
  --sidecars-dir "$WK_PROOF_ROOT/target/debug" \
  --target aarch64-unknown-linux-musl
docker image inspect rust:1 --format '{{.Id}} {{.Architecture}}/{{.Os}}'
```

If any public Docker prerequisite is absent or incompatible, the runner records
`docker_gate_boundary` as `unproven` with no commands executed. It still runs
the other credential-free groups. The three public commands set
`DEADRECKON_LIVE_DOCKER_TEST=1` themselves and never invoke a paid provider.

The result must name `WK_SOURCE_REV` and record `repository.dirty` as `false`.
Only after checking those fields should the operator copy the result into the
detached worktree and commit it on an evidence branch:

```sh
WK_EVIDENCE_BRANCH="watchkeeper-evidence-$(git rev-parse --short "$WK_SOURCE_REV")"
git -C "$WK_SOURCE_WORKTREE" switch -c "$WK_EVIDENCE_BRANCH"
cp "$WK_PROOF_RESULT" \
  "$WK_SOURCE_WORKTREE/examples/watchkeeper-dogfood/credential-free-results.json"
git -C "$WK_SOURCE_WORKTREE" add \
  examples/watchkeeper-dogfood/credential-free-results.json
git -C "$WK_SOURCE_WORKTREE" commit \
  -m "test(watchkeeper): record clean adversarial evidence"
test "$(git -C "$WK_SOURCE_WORKTREE" rev-parse HEAD^)" = "$WK_SOURCE_REV"
```

This produces a clean tested source commit `S` and a separate evidence commit
`E`. The JSON in `E` continues to name `S`; later descendant commits do not
invalidate that source binding. `E` cannot name its own commit hash because
adding that hash to the JSON changes `E` and therefore changes the hash again.
Git parentage, or a later signed attestation, identifies `E`.

The checked-in result records 13 passing credential-free proof groups, no
failures and nine unproven live claims against clean source `a0d262d`; evidence
commit `e1d0825` has that source as its parent. Later functional changes still
require a fresh source/evidence pair before inheriting those claims.

The proof groups directly cover supported
creation routes entering one Job journey, one- and two-round Graph semantic
parent repair, one-round Campaign semantic parent repair, candidate-ready
recovery, and all seven repair-receipt lineage attacks. They also prove that
Job-owned child Runs cannot bypass their parent lifecycle and that hostile
read-only planning cannot write to the operator workspace while a benign
planner remains usable under an operational sandbox. An opt-in live Docker
trial additionally proves the real container's key, environment, network and
control-path boundary when `rust:1` is already cached; it never pulls an image
implicitly. A second Docker group now runs three public strict Jobs: normal
deterministic completion, operator cancellation, and worker `SIGKILL` followed
by stale-container cleanup and exactly one retry. Those three rows are included
in the checked clean-source evidence described above.

The JSON output records each command, duration, exit status, matched test line,
and stdout/stderr digest. It labels proof by scope. The runner first proves
that `sandbox-exec` can apply a profile; nested environments that deny that
operation report the host-sandbox trials as `unproven`. The network test
proves a local server is reachable without containment and unreachable through
DeadReckon's Seatbelt profile. The common Docker control-boundary pass and the
public strict Docker group are distinct: the latter requires the static Linux
evaluator sidecar and verifies Job-level completion, cancellation and recovery.
Neither host-backed pass claims the stronger `live_docker_gate_attack`, a live
provider, an active user service or a reboot. The credential-free
`docker_gate_boundary` ID is intentionally separate from that hostile-worker,
independent-judge and valid-receipt claim.

The following remain standing live claims until an operator performs them:

- worker death during an approved live provider run;
- supervisor death during an approved live provider run;
- host network loss during a live provider call;
- a real machine reboot with the user service active;
- a cross-provider hostile gate trial from the 24-task matrix;
- a naturally occurring live semantic `revise` followed by Graph or Campaign
  parent repair;
- live Campaign interruption in which a replacement owner adopts the same
  authenticated sub-Plan process launch and recovers it without a second
  persisted launch fact;
- the equivalent protected gate boundary on Linux/bubblewrap.
- a hostile live Docker worker paired with a distinct independent judge and a
  valid Docker-bound completion receipt (`live_docker_gate_attack`).

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

`live_provider_network_loss` is narrower: its worker must be exactly one
registry-backed HTTP route. Prepare derives and signs that route's non-loopback
endpoint from the provider registry; there is no caller-provided probe URL.
Capture `network-reachable-before`, apply the reviewed host change, record the
intervention while the same supervised attempt is still current, restore the
change, then capture `network-reachable-after`. The helper accepts only the
official probe's `endpoint_unreachable` result for the middle observation and
does not retain its free-form error text or credentials. The evidence proves
an observed route outage and ordered Job response, not causal attribution to
the host command.

`live_campaign_interruption_recovery` binds the one active sub-Plan's complete
prepared, released and linked process authority before interruption. After the
parent lease is reclaimed, its canonical intervention is exactly one
`sub_process_adopted` event for the same parent, sub, Plan, attempt, launch,
PID, boot and process-start identities under the newer lease. The oracle then
requires `sub_recovered`, exact append-only Campaign and Plan histories, no
second launch fact and no reopened completed Plan task. This is an objective
persisted-adoption claim, not a promise that arbitrary external side effects
are globally exactly-once.

Every trial declares the exact terminal outcome/reason pairs that count as an
acceptable product response before the intervention. Verified work still has
to carry the normal valid completion receipt. A declared non-Verified response,
such as `needs_review/semantic_unavailable`, is accepted only when the protected
helper signs the matching final Job history and confirms that no completion
receipt exists. Similar-looking but undeclared pairs fail closed.

```sh
export WK_TRIAL_ID=live_provider_supervisor_restart
export WK_LIVE_TRIAL="$DEADRECKON_DOGFOOD_ARTIFACTS/live/$WK_TRIAL_ID-01"
export WK_JOB_ID=the-full-approved-job-id
export DR_CAPTURE_BIN=/protected/deadreckon/bin/dr-capture
export DEADRECKON_BIN=/protected/deadreckon/bin/deadreckon
export WK_JOB_AUTHORITY="$DEADRECKON_HOME/jobs/$WK_JOB_ID/authority.json"
test -f "$WK_JOB_AUTHORITY"
test ! -L "$WK_JOB_AUTHORITY"
export WK_JOB_SOURCE_REV="$(
  python3 -c \
    'import json,re,sys; value=json.load(open(sys.argv[1], encoding="utf-8"))["source_revision"]; assert isinstance(value, str) and re.fullmatch(r"[0-9a-f]{40}", value); print(value)' \
    "$WK_JOB_AUTHORITY"
)"

python3 examples/watchkeeper-dogfood/live-trial.py prepare \
  "$WK_TRIAL_ID" \
  --trial-dir "$WK_LIVE_TRIAL" \
  --revision "$WK_JOB_SOURCE_REV" \
  --capture-helper "$DR_CAPTURE_BIN" \
  --deadreckon-binary "$DEADRECKON_BIN" \
  --job-id "$WK_JOB_ID" \
  --backend sandbox-exec \
  --provider-route worker=cli:codex \
  --provider-route independent_judge=cli:claude-code
python3 -m json.tool "$WK_LIVE_TRIAL/replay.json"
```

Prepare is retry-safe: the same inputs reuse the same capture session and
protected binding after an interruption. `WK_JOB_SOURCE_REV` is the approved
target Job source revision, not the revision of the checkout containing the
recorder. The protected helper independently binds and revalidates the Job
authority, so a missing or different revision is refused. Conflicting inputs
are refused.
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
