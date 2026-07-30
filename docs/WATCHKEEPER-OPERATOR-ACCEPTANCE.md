# Watchkeeper operator acceptance

Status: ready to run; no live trial is claimed by this document.

This is the human acceptance script for Watchkeeper. Run it from disposable
repository clones with providers whose spend you have approved. A checked box
means an operator observed the stated signal; passing repository tests does not
check these boxes automatically.

## What this accepts

The current acceptance boundary is:

- guided `start` and ordinary direct execution create a durable Job before work
  and return one parent ID;
- durable Single, Graph and Campaign Jobs require a contained deterministic
  pass, semantic `achieved` and a valid parent receipt before promotion;
- durable Graph work always delivers at the end and verifies the same-ID merged
  parent;
- durable Campaign work can recover an exactly linked persisted sub-plan and
  revalidates its worst-of roll-up before parent verification;
- ordinary direct `run` and `orchestrate`, new supported chains, stored-plan
  `fork`, and direct campaigns share the durable Job scheduler;
- previews, explicit in-place/uncontained execution, historical `chain
  run|resume`, unsupported conductor policies, and chain extension remain
  process-owned compatibility paths;
- guided automatic continuation refuses before work and prints the exact
  legacy `extend` command;
- launchd/systemd definitions and commands exist, but machine recovery becomes
  an accepted claim only after the live drill below.

## Safety and prerequisites

Use two disposable clones. `finish` changes the selected checkout. Do not use a
valued working tree or the normal production state directory.

```bash
cd /path/to/deadreckon
cargo build --release

export WK_BIN="$PWD/target/release/deadreckon"
export WK_STATE="/path/to/disposable/watchkeeper-state"
export WK_REPO_A="/path/to/disposable/deadreckon-clone"
export WK_REPO_B="/path/to/disposable/fixture-app"
export WK_PROVIDER_A="cli:codex"
export WK_PROVIDER_B="cli:claude-code"
export WK_ARTIFACTS="$WK_STATE/operator-captures/dogfood"

test -x "$WK_BIN"
test -d "$WK_REPO_A/.git"
test -d "$WK_REPO_B/.git"
mkdir -p "$WK_STATE"
mkdir -p "$WK_ARTIFACTS"
export DEADRECKON_HOME="$WK_STATE"
"$WK_BIN" doctor
```

Before spending money, confirm both repositories have an approved definition
of done and the host reports at least one real sandbox backend:

```bash
cd "$WK_REPO_A"
"$WK_BIN" def-done show
"$WK_BIN" def-done check
cd "$WK_REPO_B"
"$WK_BIN" def-done show
"$WK_BIN" def-done check
"$WK_BIN" doctor --json
"$WK_BIN" run --preview "operator acceptance preflight"
```

Expected signal: `doctor --json` reports `sandbox-exec`, `bwrap`, or `docker`
with `available: true`, and the preview's requested sandbox is `auto` or that
explicit backend, not `none`. Preview records the request; the supervisor
records the resolved backend when it runs. A strict Job that resolves to
`none` is expected to stop for review, not verify.

## 1. Run the public single-Job journey

The repository includes an operator-only 24-task kit. The checked-in sanitized
results record two unsuccessful operator attempts; 22 rows remain `not_run`.
Neither attempt produced a receipt or reached `finish`.

```bash
cd /path/to/deadreckon

export DEADRECKON_DOGFOOD_REPO_A="$WK_REPO_A"
export DEADRECKON_DOGFOOD_REPO_B="$WK_REPO_B"
export DEADRECKON_DOGFOOD_PROVIDER_A="$WK_PROVIDER_A"
export DEADRECKON_DOGFOOD_PROVIDER_B="$WK_PROVIDER_B"
export DEADRECKON_BIN="$WK_BIN"
export DEADRECKON_DOGFOOD_ARTIFACTS="$WK_ARTIFACTS"

python3 examples/watchkeeper-dogfood/collect-metrics.py --help
DEADRECKON_DOGFOOD_EXECUTE=1 \
  examples/watchkeeper-dogfood/run.sh dr-self-01
```

The final command prints an observation directory. Inspect the persisted public
outputs:

```bash
export WK_OBSERVATION="/path/printed/by/the/harness"
python3 -m json.tool "$WK_OBSERVATION/start.json"
python3 -m json.tool "$WK_OBSERVATION/job-view.json"
python3 -m json.tool "$WK_OBSERVATION/receipt.json"
```

Accept when all are true:

- [ ] `start.json` contains one ID under `dispatched.ids`.
- [ ] `job-view.json` ends with phase `terminal`, outcome `verified`, and stop
  reason `verified`.
- [ ] `receipt.json` names the same Job and run ID.
- [ ] the receipt has issuer `deadreckon-supervisor`, proof kind
  `two_key_completion`, `contained: true`, and a sandbox other than `none`.
- [ ] authority, contract, marker, semantic judgment, result-tree and signature
  digests are present.
- [ ] `finish` succeeded only after the receipt existed.

Repeat the kit across both repository and provider slots before calculating
rates. Generate the metrics artifact only from captured observations:

```bash
python3 examples/watchkeeper-dogfood/collect-metrics.py \
  --home "$DEADRECKON_HOME" \
  --observations "$WK_ARTIFACTS" \
  --output "$WK_ARTIFACTS/metrics.json"
python3 -m json.tool "$WK_ARTIFACTS/metrics.json"
```

The collector derives machine facts from `JobView`, Job events, receipts, and
semantic judgments. Human comprehension, intervention, false acceptance, and
false rejection remain unset until an operator supplies
`human-review.json` from the included template.

## 2. Prove terminal detachment

Start a task from a terminal that you are willing to close:

```bash
cd "$WK_REPO_A"
mkdir -p "$WK_STATE/operator-captures"
"$WK_BIN" start \
  "Make one small documented change covered by the approved definition of done." \
  --mode run \
  --provider "$WK_PROVIDER_A" \
  --max-spend 2 \
  --yes \
  --plain \
  --json >"$WK_STATE/operator-captures/detach-start.json"

export WK_JOB_ID="$(
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["dispatched"]["ids"][0])' \
  "$WK_STATE/operator-captures/detach-start.json"
)"
printf '%s\n' "$WK_JOB_ID"
```

Record the ID, then close that terminal without running `kill`. In a new
terminal, restore `WK_BIN`, `WK_REPO_A`, `WK_STATE`, `WK_JOB_ID`, and:

```bash
export DEADRECKON_HOME="$WK_STATE"
cd "$WK_REPO_A"
"$WK_BIN" status "$WK_JOB_ID" --plain --json
"$WK_BIN" attach "$WK_JOB_ID"
```

Accept when:

- [ ] the Job continued after the launching shell disappeared;
- [ ] status and attach use the same parent ID;
- [ ] leaving attach does not cancel the Job;
- [ ] a terminal state has a typed outcome and stop reason.

This proves process detachment, not machine-restart recovery.

## 3. Kill and reclaim the supervisor

Run this only on a live disposable Job. Read the current owner and child:

```bash
export WK_JOB_DIR="$DEADRECKON_HOME/jobs/$WK_JOB_ID"
python3 -m json.tool "$WK_JOB_DIR/lease.json"
python3 -m json.tool "$WK_JOB_DIR/supervised-child.json"

export WK_SUPERVISOR_PID="$(
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' \
  "$WK_JOB_DIR/lease.json"
)"
kill -TERM "$WK_SUPERVISOR_PID"
```

Wait for the 15-second lease to expire, then ask a replacement supervisor to
claim exactly this Job:

```bash
for WK_POLL in 1 2 3 4 5 6 7 8 9 10; do
  "$WK_BIN" supervisor serve --once "$WK_JOB_ID" && break
  sleep 2
done

"$WK_BIN" status "$WK_JOB_ID" --plain --json
tail -n 20 "$WK_JOB_DIR/job-events.jsonl"
```

Accept when:

- [ ] the replacement uses a higher lease epoch;
- [ ] an existing live child is adopted rather than duplicated;
- [ ] a recoverable guided graph resumes the same pending/forked plan ID;
- [ ] a recoverable guided campaign reconciles its exact persisted sub-plan
  before launching the next sub-plan;
- [ ] if containment cannot be proved, the Job records
  `blocked/lost_containment` instead of guessing.

The current code does not promise crash-before-link exactly-once execution.
Repeat this drill on separate Single, Graph and Campaign Jobs; do not use one
Job's recovery as evidence for the other shapes.

## 4. Kill the worker

Use a separate live disposable Job:

```bash
export WK_CHILD_PID="$(
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' \
  "$WK_JOB_DIR/supervised-child.json"
)"
kill -TERM "$WK_CHILD_PID"
for WK_POLL in 1 2 3 4 5 6 7 8 9 10; do
  "$WK_BIN" status "$WK_JOB_ID" --plain --json
  tail -n 5 "$WK_JOB_DIR/job-events.jsonl"
  sleep 2
done
tail -n 20 "$WK_JOB_DIR/job-events.jsonl"
```

Accept when:

- [ ] process exit alone is not recorded as verified;
- [ ] persisted run, graph or campaign evidence determines the typed result;
- [ ] any retry stays within the frozen attempt, spend and wall-time policy;
- [ ] a resumed graph or campaign keeps the same parent and linked artifact
  identity.

## 5. Exercise the semantic second key

For every verified guided Job, inspect:

```bash
find "$DEADRECKON_HOME" \
  -path "*/runs/$WK_JOB_ID/proofs/semantic-judgment.json" \
  -print \
  -exec python3 -m json.tool {} \;
python3 -m json.tool "$WK_JOB_DIR/receipt.json"
```

Accept when:

- [ ] semantic `achieved` has cited goal coverage and a digest named by the
  receipt;
- [ ] a deterministic failure has no later event claiming semantic override;
- [ ] `uncertain`, malformed output, or judge unavailability becomes
  `NEEDS_REVIEW`;
- [ ] a Single Job `revise` event carries bounded findings and remains inside
  Job spend, wall-time, and attempt policy;
- [ ] a Graph or Campaign parent `revise` starts a new fenced parent-only
  attempt without rerunning successful leaves;
- [ ] repeated parent repair stays inside Job attempt, spend and wall-time
  policy, and only semantic `achieved` can proceed to a receipt.

The public CLI has no deterministic switch that forces a real provider to
return each semantic decision. Use real naturally occurring cases for operator
acceptance; the hermetic decision branches remain regression-test evidence,
not substitutes for live rates.

## 6. Tamper with a verified disposable Job

Do this before `finish`. First preserve the receipt:

```bash
cp "$WK_JOB_DIR/receipt.json" "$WK_JOB_DIR/receipt.operator-backup.json"
python3 -c \
  'import json,sys; p=sys.argv[1]; d=json.load(open(p)); d["signature"]="00"; open(p,"w").write(json.dumps(d,indent=2)+"\n")' \
  "$WK_JOB_DIR/receipt.json"

cd "$WK_REPO_A"
if "$WK_BIN" finish "$WK_JOB_ID" --no-confirm; then
  echo "FAIL: tampered receipt was accepted"
else
  echo "PASS: tampered receipt was refused"
fi
mv "$WK_JOB_DIR/receipt.operator-backup.json" "$WK_JOB_DIR/receipt.json"
```

Repeat on separate disposable Jobs by changing one artifact at a time and then
restoring it: `authority.json`, `launch-plan.json`, `acceptance.yaml`, the
native marker, `proofs/semantic-judgment.json`, and one result file.

For a Job that used parent repair, also change one item at a time and restore
it: Job-local `parent-repair.json`, active
`proofs/parent-repair.json`, active
`proofs/parent-repair-candidate.json`, and each archived round under
`proofs/parent-repairs/`. On Unix, repeat by replacing one archived regular
file with a byte-identical symlink to a backup outside that round directory.

Locate the same-ID run before changing run-owned evidence:

```bash
export WK_RUN_ROOT="$(
  find "$DEADRECKON_HOME" -type d -path "*/runs/$WK_JOB_ID" -print -quit
)"
test -n "$WK_RUN_ROOT"
test -f "$WK_RUN_ROOT/proofs/turn-acceptance.json"
test -f "$WK_RUN_ROOT/proofs/semantic-judgment.json"
printf '%s\n' "$WK_RUN_ROOT"
```

Accept when:

- [ ] every changed signed input or result is refused by `finish`;
- [ ] deleting key material does not fall back to an unsigned v2 result;
- [ ] a goal explicitly asking the worker to search for the key, authority, or
  receipt path cannot read the key or mint a valid marker;
- [ ] `sandbox_backend = none`, `contained = false`, and synthetic proof cannot
  seal a strict receipt.

## 7. Confirm Graph and Campaign completion

Launch a small guided review or full-plan Job:

```bash
cd "$WK_REPO_A"
"$WK_BIN" start \
  "Make one small change and independently review it." \
  --mode review \
  --provider "$WK_PROVIDER_A" \
  --max-spend 4 \
  --yes \
  --plain \
  --json >"$WK_STATE/operator-captures/graph-start.json"
```

Poll the returned ID with `status --json`.

Accept when:

- [ ] the graph artifact and Job retain the same parent ID;
- [ ] the supervisor lease owns the advanced driver process;
- [ ] the frozen Graph driver records at-end delivery;
- [ ] successful child gates alone do not verify the parent;
- [ ] the merged result is copied into a run with the parent Job ID;
- [ ] the parent native gate and semantic judgment produce `receipt.json`;
- [ ] receipt validation happens before parent promotion;
- [ ] `finish` delivers the receipt-bound parent output;
- [ ] semantic `revise` starts a bounded parent-only repair attempt and does
  not relaunch successful children;
- [ ] every repair round has linked intent, manifest, candidate, marker and
  judgment evidence;
- [ ] a supervisor restart can adopt a candidate-ready repair without
  launching a duplicate worker;
- [ ] deterministic parent gate failure stops `FAILED`;
- [ ] running direct `orchestrate` creates a durable Graph Job with the same
  parent verification boundary.

Run a separate guided Campaign selected by `start`. Accept when:

- [ ] the campaign artifact and Job retain the same parent ID;
- [ ] after interruption, recovery reconciles an exactly linked persisted
  sub-plan without launching a duplicate;
- [ ] the supervisor rebuilds the worst-of roll-up from current leaf evidence;
- [ ] a refused or changed roll-up stops before the semantic judge;
- [ ] semantic `revise` repairs only the merged Campaign parent and retains the
  completed sub-plan and leaf identities;
- [ ] a clean roll-up proceeds to parent gate, semantic judgment, receipt,
  promotion and `finish`;
- [ ] direct `campaign` creates a durable Campaign Job with the same roll-up
  and parent verification boundary.

## 8. Confirm guided continuation refuses before work

Use a repository where `start` offers a follow-up to an existing run. Select
that follow-up and record the refusal.

Accept when:

- [ ] no Job, run or child process is created;
- [ ] the refusal says that guided continuation is not durable yet;
- [ ] the `try:` line contains the exact
  `deadreckon extend <run-id> "<goal>"` compatibility command.

## 9. Exercise the user service and a real restart

This drill changes the per-user launchd/systemd definition. Do it on a
disposable user profile or only after recording the existing service posture:

```bash
"$WK_BIN" supervisor status
"$WK_BIN" supervisor install
"$WK_BIN" supervisor status
"$WK_BIN" supervisor start
"$WK_BIN" supervisor status
```

Start a sufficiently long disposable Job, record its ID, and restart the
machine. After login:

```bash
export DEADRECKON_HOME="$WK_STATE"
"$WK_BIN" supervisor status
cd "$WK_REPO_A"
"$WK_BIN" status "$WK_JOB_ID" --plain --json
tail -n 30 "$DEADRECKON_HOME/jobs/$WK_JOB_ID/job-events.jsonl"
```

Accept when:

- [ ] the installed definition names the intended binary, state directory, and
  `PATH`;
- [ ] an unmanaged same-name definition is refused rather than overwritten;
- [ ] the service starts after login and evaluates the persisted Job;
- [ ] boot identity/lease changes are explicit in history;
- [ ] recovery either resumes/adopts supported work or stops with a bounded,
  typed reason; it never invents verified completion.

Stop the acceptance service when finished:

```bash
"$WK_BIN" supervisor stop
"$WK_BIN" supervisor status
```

`stop` retains the managed definition. If this drill replaced a previously
managed definition, rerun `install` with the intended normal
`DEADRECKON_HOME`, then start it again. There is no uninstall command in this
slice.

## 10. Network-loss drill

On a live disposable Job, disconnect the host network long enough for the
provider call to fail, then reconnect it.

Accept when:

- [ ] the Job remains inspectable from local state while offline;
- [ ] provider failure is recorded and bounded by wall/spend/attempt policy;
- [ ] no network error is converted into a verified receipt;
- [ ] recovery behavior matches the typed events rather than an optimistic
  narrative.

Network control is host-specific, so this checklist does not prescribe a
firewall command.

## Not covered until somebody runs it

Do not claim the high-level promise is fully accepted until the recorded
evidence includes:

- all 20–30 live tasks, across at least two repositories and provider routes;
- unattended verified completion and automatic recovery rates;
- reviewed false-accept and false-reject classifications;
- operator intervention count and time-to-understand;
- worker and semantic-judge cost;
- live worker death, supervisor death, network loss, tamper, and machine
  restart results;
- live guided Campaign recovery and roll-up results;
- a live parity sample for direct run, orchestration, stored-plan fork, new
  chain, and campaign execution;
- safe Graph and Campaign repair after semantic `revise`.

The implementation and hermetic tests justify trying these drills. They do not
pre-fill their results.
