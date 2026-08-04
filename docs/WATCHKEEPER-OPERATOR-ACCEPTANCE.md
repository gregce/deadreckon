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
- public `extend` and a follow-up selected by guided `start` create a durable
  parent-bound Single Job;
- previews, explicit in-place/uncontained execution, historical `chain
  run|resume`, unsupported conductor policies, and chain extension remain
  process-owned compatibility paths;
- launchd/systemd definitions and commands exist, but machine recovery becomes
  an accepted claim only after the live drill below.

## Current phase quick acceptance

This short script accepts the durable Docker cleanup and complete release
payload added in the current phase. It is safe to run from this repository: the
tests create disposable Jobs and delete only containers whose labels match
those Jobs.

From an arm64 Mac with Docker running and `rust:1` already cached:

```bash
cd /path/to/deadreckon

test -x target/debug/deadreckon
test -x target/debug/dr-gate
test -x target/debug/dr-capture
test -x target/debug/dr-gate-evaluator-aarch64-unknown-linux-musl
file target/debug/dr-gate-evaluator-aarch64-unknown-linux-musl
docker image inspect rust:1 --format '{{.Id}} {{.Architecture}}/{{.Os}}'

DEADRECKON_LIVE_DOCKER_TEST=1 \
  cargo test -p deadreckon \
  --test watchkeeper_trust_boundary \
  live_docker_ \
  -- --ignored --nocapture

docker ps -a \
  --filter label=io.deadreckon.managed=true \
  --format '{{.ID}} {{.Status}} {{.Names}}'

cargo test -p deadreckon \
  --test release_plan \
  --test npm_wrapper
```

Accept when:

- [ ] `file` reports a statically linked ARM aarch64 ELF evaluator;
- [ ] Docker reports a `sha256:` image ID and `arm64/linux`;
- [ ] the three `live_docker_` tests pass;
- [ ] the managed-container query prints nothing after the tests;
- [ ] `release_plan` and `npm_wrapper` pass, including archive assembly,
  member-manifest, installer, Homebrew and npm payload assertions.

This quick script does not accept a real semantic `achieved`, Linux/bubblewrap,
machine reboot, 20–30-task dogfood matrix, Apple notarization, Windows
Authenticode signing, npm publication or GitHub attestation. Those require the
later operator and protected-CI steps.

### Same-version stale-helper drill

Before a stable cut, prove that a locally rebuilt controller cannot silently
freeze an older helper which happens to report the same package version:

```bash
cargo build --release --workspace --locked
target/release/dr-gate protocol
cargo test -p deadreckon \
  gate_helper_requires_protocol_and_exact_bundle_build_identity \
  --bin deadreckon
cargo test -p deadreckon --test release_plan evaluator_sidecar_tool_ -- --nocapture
```

Accept when:

- [ ] `protocol` reports the expected gate protocol, package version and one
  `deadreckon-bundle-build-id-sha256:` identity;
- [ ] a same-protocol helper with a different bundle identity is rejected;
- [ ] release assembly rejects mixed controller, native-gate or evaluator
  sidecar build identities;
- [ ] `deadreckon doctor` reports a missing or incompatible gate helper as a
  blocking binary-bundle failure, not a healthy installation;
- [ ] no rejected Job reaches `Queued`, and no frozen authority is rewritten to
  repair an existing Job.

## 0.8.1 start-authoring regression acceptance

Use this short drill to accept the Codex schema, retired-feature, supervisor
boot-identity and phase-timing corrections. It uses a disposable repository but
the selected provider may consume subscription quota.

```bash
cd /path/to/deadreckon
cargo build --release --workspace --locked
export WK_BIN="$PWD/target/release/deadreckon"
export WK_081_FIXTURE="$(mktemp -d)"

cd "$WK_081_FIXTURE"
git init
git config user.email watchkeeper@example.invalid
git config user.name Watchkeeper
printf '%s\n' 'print("hello from app")' > app.py
git add app.py
git commit -m fixture

"$WK_BIN" def-done \
  "running python3 app.py prints hello from app" \
  --provider cli:codex \
  --model gpt-5.6-sol
"$WK_BIN" def-done show
"$WK_BIN" def-done check
"$WK_BIN" supervisor status
```

Then remove the disposable contract files and run interactive `start` with a
small goal. Choose Codex and `gpt-5.6-sol`, enter the same one-line done
criterion, approve the generated contract and launch the Job. If authoring is
deliberately interrupted or the provider is made unavailable, verify that the
recovery prompt offers retry, revise and stop without asking for the source,
launch shape, provider or model again.

The timing boundary for this drill is one 20-minute automated admission-work
clock shared by goal-shape planning, draft, critic and an optional redraft. Each
automated phase receives only the current remainder; operator prompts pause this
work clock, while an explicit calendar deadline does not. No phase gets a fresh
clock, a reserved slice or an extension. An earlier explicit Job deadline wins.
Provider cleanup gets a
separate 30-second proof window but cannot authorize fallback or extend the
approved work cutoff. Standalone compatibility commands retain bounded route
safety limits and cancel through the same cleanup shape.

Accept when:

- [ ] neither command reports `web_search_request` as deprecated;
- [ ] neither command reports `invalid_json_schema`, a `files` required-key
  mismatch, or a dynamic `additionalProperties` map;
- [ ] the generated contract contains a behavioral shell check and `check`
  passes;
- [ ] a clean worktree remains admissible after `start` writes the approved
  `.deadreckon/acceptance.yaml` and `.md`; the supervised child starts, reaches
  a persisted run state, and does not call those controller files user dirt;
- [ ] `deadreckon def-done "behavior"` followed by its recommended
  `deadreckon start "goal" --worktree --yes` command accepts the exact bounded
  project contract without requiring `--allow-dirty`; adding any unrelated
  uncommitted file restores the dirty-worktree refusal;
- [ ] a generated helper under `.deadreckon/acceptance/` is copied into the
  Job's bounded contract bundle, appears in the launch-plan manifest, reaches
  the isolated worktree, and executes there;
- [ ] changing the generated YAML or adding an unfrozen helper/unrelated file
  still fails closed, and a pre-state child failure points to `supervisor.err`
  without claiming the provider caused it;
- [ ] an authoring failure never suggests setting the provider to the same
  route that just failed;
- [ ] the guided recovery keeps the already selected source, mode, provider
  and model;
- [ ] `supervisor status` uses the same `kern.bootsessionuuid` across repeated
  probes even when `kern.boottime` changes, accepts legacy time checkpoints
  only against another same-second legacy identity, and requires one supervised
  restart when upgrading a legacy checkpoint to the UUID format;
- [ ] rebuilding or replacing the supervisor executable at the same path makes
  the old live process fail readiness by bundle build identity or executable
  SHA-256; guided start repairs the service and proves a fresh checkpoint
  before creating a Job;
- [ ] no supervised PID record is removed when timeout cleanup cannot be
  proven;
- [ ] unresolved goal-shape provider cleanup stops before a Job ID is created;
  deterministic shape fallback occurs only after the provider tree is reaped;
- [ ] goal-shape planning and all contract stages share one 20-minute automated
  admission-work allowance and inherit only its remainder rather than starting a
  fresh clock or dividing it into phase-local shares; operator prompt time does
  not consume the allowance;
- [ ] an absolute deadline that elapses during admission creates no queued Job
  and tells the operator to choose a later deadline;
- [ ] after suspending and waking the host beyond one lease heartbeat window,
  a still-live same-boot supervisor retains its lease epoch and owner; a second
  supervisor does not reclaim the Job merely because wall clock advanced;
- [ ] Job wall, spend and absolute deadline values remain the operator-approved
  values; only setup/readiness/inactivity allowances are relaxed.
- [ ] on macOS, add a required shell check that captures product output with
  bare `mktemp`; the check completes under Seatbelt and its temporary file is
  created beneath the disposable gate runtime, not the Darwin user temp root;
- [ ] a newly generated contract never emits bare `mktemp` or `mktemp -t` and
  `def-done` rejects a provider draft that does not use an explicit
  `${TMPDIR:-/tmp}/...XXXXXX` template;
- [ ] shell checks observe DeadReckon's isolated `HOME`, `TMPDIR` and `PATH`
  even when the operator's shell startup files assign different values.
- [ ] if a deterministic check fails and the corrective provider turn exits
  cleanly without a deliverable change, status retains the exact acceptance
  failure and records `deterministic_revise`, never `fatal_provider`;
- [ ] a deadline or wall-cap reached with an unverifiable live child returns
  within the 30-second cleanup boundary as `Blocked/LostContainment` and keeps
  the process-authority record for recovery.
- [ ] after restarting the singleton supervisor, a second pending Job starts
  while an earlier recovered Job is still running; no more than four recovery
  Jobs are driven concurrently;
- [ ] a contained read-only Codex request can read its frozen output schema but
  cannot modify it, including through the schema's canonical path, and its
  precreated last-message output remains visible when Linux gives the sandbox a
  private `/tmp`;
- [ ] a documentation provider that hangs with a TERM-ignoring descendant is
  killed and reaped at its call boundary, leaves no child authority, and keeps
  the deterministic narrative; unproved cleanup fails the run instead;
- [ ] a timed-out hook that leaves a pipe-holding descendant cannot extend its
  configured timeout indefinitely. Proven cleanup follows the configured seam
  fallback; unproved cleanup reports `LostContainment` and stops dispatch;
- [ ] semantic judge cancellation reaps its complete process group and removes
  its identity record. If that cannot be proved, leaf, graph and campaign Jobs
  stop as `Blocked/LostContainment`, never `NEEDS_REVIEW` or `fatal_provider`.
- [ ] a semantic judge that finds only a cosmetic, non-blocking observation
  keeps it in `summary`, returns `achieved` with empty `blocking_missing`, and
  seals normally; a genuinely blocking item requires `revise` or `uncertain`,
  while any still-contradictory response fails closed as `NEEDS_REVIEW`.

Exercise the timing assertions at every phase boundary, not only provider
mutation. For admission drafting, goal-shape planning, provider turns, tool
execution, documentation, deterministic verification, semantic judging,
promotion, root planning, child scheduling and recovery, verify this matrix:

- [ ] retries and supervisor restarts reuse the original absolute work cutoff;
- [ ] a phase does not launch when less than its minimum usable work interval
  remains, and fractional time is never rounded up into extra authority;
- [ ] reaching the work cutoff cancels new work immediately, then grants only
  the separate cleanup window to reap and prove the owned process tree;
- [ ] proven cleanup produces the phase's typed bounded stop reason, while
  unproved cleanup produces `Blocked/LostContainment` and retains authority;
- [ ] state saved after a failed snapshot, Git, documentation, gate or
  promotion boundary includes the time already consumed;
- [ ] run wall accounting is monotonic and controller-measured; provider or
  judge timing evidence is not added to it a second time.
- [ ] Plan, Chain and Campaign children inherit the owning Job's remaining
  work window; parallel child durations are never summed to manufacture a
  false wall-cap exhaustion.
- [ ] the same fixed cutoff is inherited by deterministic gates, semantic
  judging, merge repair, catalog/policy/hook seams, Git work, receipt sealing
  and promotion; a calendar deadline remains typed as `deadline` rather than
  being rewritten as `wall_cap`.

## Safety and prerequisites

Use two disposable clones. `finish` changes the selected checkout. Do not use a
valued working tree or the normal production state directory.

```bash
cd /path/to/deadreckon
cargo build --release --workspace --locked

export WK_BIN="$PWD/target/release/deadreckon"
export WK_STATE="/path/to/disposable/watchkeeper-state"
export WK_REPO_A="/path/to/disposable/deadreckon-clone"
export WK_REPO_B="/path/to/disposable/fixture-app"
export WK_PROVIDER_A="cli:codex"
export WK_PROVIDER_B="cli:claude-code"
export WK_ARTIFACTS="$WK_STATE/operator-captures/dogfood"
export DR_CAPTURE_BIN="$PWD/target/release/dr-capture"
export DEADRECKON_BIN="$WK_BIN"

test -x "$WK_BIN"
test -x "$DR_CAPTURE_BIN"
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

## Record every live fault claim

Prepare the passive recorder after creating the disposable Job for each fault
drill and before the intervention. It records evidence but never starts a
provider, signals a process, changes networking, controls the service, reboots,
or calls `finish`. The operator remains responsible for the reviewed fault and
cleanup.

Pass-capable recording requires the protected `dr-capture` and sibling
`deadreckon` binaries to remain outside every Job source, working, run, merge
and repair root. For example, before section 3:

```bash
export WK_TRIAL_ID=live_provider_supervisor_restart
export WK_LIVE_TRIAL="$WK_ARTIFACTS/live/$WK_TRIAL_ID-01"
export WK_JOB_ID=the-full-approved-job-id
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
  --provider-route worker="$WK_PROVIDER_A" \
  --provider-route independent_judge="$WK_PROVIDER_B"
python3 -m json.tool "$WK_LIVE_TRIAL/replay.json"
```

Use the backend that the strict Job actually resolved. The provider roles must
match the manifest and the worker and independent judge must be different.
The revision is the approved target Job source revision from `authority.json`,
not the revision of the checkout containing this recorder. The protected
helper independently binds and revalidates that authority; a missing or
different revision is refused. Preparation is retry-safe for identical inputs
and refuses a conflicting binding.

`replay.json` lists the exact canonical subjects, reviewed intervention and
cleanup. Record the supervisor trial's `before` evidence first:

```bash
python3 examples/watchkeeper-dogfood/live-trial.py observe \
  --trial-dir "$WK_LIVE_TRIAL" \
  --canonical job-view-before \
  --canonical events-before \
  --canonical lease-before \
  --canonical supervised-child-before
```

Perform the reviewed intervention, then record its boundary separately. The
detail file is operator context; `dr-capture` independently reads the
trial-specific Job, Campaign or sandbox observation:

```bash
python3 examples/watchkeeper-dogfood/live-trial.py observe \
  --trial-dir "$WK_LIVE_TRIAL" \
  --intervention-status performed \
  --intervention-detail-file /path/to/operator-intervention-evidence
```

Record the declared `after` evidence:

```bash
python3 examples/watchkeeper-dogfood/live-trial.py observe \
  --trial-dir "$WK_LIVE_TRIAL" \
  --canonical job-view-after \
  --canonical events-after \
  --canonical lease-after \
  --canonical supervised-child-after \
  --canonical job-report
```

After the manifest's cleanup steps, finalize the deterministic evaluation and
the minimal publication envelope:

```bash
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

The envelope's HMAC publication proof authenticates the exact protected
binding, append-only evidence history, deterministic evaluation and capture
receipt. The binding approves exact outcome/reason pairs, not independent
outcome and reason lists. `verified/verified` remains valid only with the
normal `CompletionReceipt`; an approved non-Verified result requires signed
terminal-history lineage and no completion receipt. JSON schema validation
alone is not verification. Omitting the
trusted prepare arguments retains the manual compatibility path: it accepts
operator-selected `--capture NAME=PATH` files but can never produce
`status: passed`. A missing capture, unperformed intervention or missing
cleanup is never rounded up to a pass.

Use these manifest IDs:

- section 3: `live_provider_supervisor_restart`;
- section 4: `live_provider_worker_kill`;
- section 5 and section 7 natural parent repair:
  `live_provider_parent_repair`;
- section 6 cross-provider attack: `cross_provider_gate_attack`;
- section 7 Campaign interruption:
  `live_campaign_interruption_recovery`;
- section 9: `machine_reboot`;
- section 10: `live_provider_network_loss`;
- explicit Linux and Docker strict-Job trials:
  `linux_bubblewrap_gate_boundary` and `live_docker_gate_attack`.

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

python3 examples/watchkeeper-dogfood/batch.py
DEADRECKON_DOGFOOD_EXECUTE=1 \
  python3 examples/watchkeeper-dogfood/batch.py --execute
```

The first command only prints the matrix plan. The second stops on the first
non-zero task result and can be rerun to resume; it skips only observations
bound to the exact matrix digest and task, repository, provider and Job
identities. The batch passes that reviewed matrix path, artifact root and
digest to each runner; a byte change after planning is a refusal before
provider execution. A successful one-task runner prints an observation directory.
Inspect the persisted public outputs:

If a task is `blocked_partial` or `blocked_invalid`, do not rerun it directly.
Inspect the existing `start.json`, latest status, and Job state first, then
archive or repair the task artifacts. The batch deliberately refuses to create
a second Job from an ambiguous task directory. The direct runner claims the
task directory atomically, so an existing empty directory also blocks a new
start. Symlinked task and Job artifact directories are rejected.

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
  --matrix examples/watchkeeper-dogfood/matrix.json \
  --output "$WK_ARTIFACTS/metrics.json"
python3 -m json.tool "$WK_ARTIFACTS/metrics.json"
```

The collector derives machine facts from `JobView`, Job events, receipts, and
semantic judgments. Human comprehension, intervention, false acceptance, and
false rejection remain unset until an operator supplies
`human-review.json` from the included template.
The collector rejects unknown or duplicate observed task IDs and reports the
missing, attempted, completed, verified and reviewed task sets. Its execution
status can become `complete` only for all 20–30 tasks across at least two
repository slots and two provider slots, with no ambiguous terminal artifacts.
Its separate assessment status becomes `ready`, and
`campaign_completion.claim_allowed` becomes true, only after every completed
task has an exact, fully populated, valid human review. The untouched template
does not count as a review. Missing factual artifacts, invalid event rows, and
symlinked or non-file Job event histories keep the assessment incomplete.

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

Before spending provider tokens, the cached-image Docker control-boundary test
can be run independently:

```bash
cd /path/to/deadreckon
docker image inspect rust:1 >/dev/null
DEADRECKON_LIVE_DOCKER_TEST=1 \
  cargo test -p deadreckon-sandbox --lib \
  tests::live_docker_denies_control_tampering_and_gate_inputs \
  -- --ignored --exact --nocapture
```

Accept when the exact test reports `ok`. This command does not pull an image.
It proves the real container denies key visibility, signing inputs, network
routes, Job/proof/gate/Git control writes and still permits an ordinary
deliverable write.

To exercise the public strict-Job path on an arm64 host, build or install the
static Linux evaluator beside the test `deadreckon` binary, then run:

```bash
DEADRECKON_LIVE_DOCKER_TEST=1 \
  cargo test -p deadreckon \
  --test watchkeeper_trust_boundary \
  live_docker_ \
  -- --ignored --nocapture
```

Accept only when all three tests pass:

- [ ] normal completion produces a contained native Docker marker with passing
  deterministic checks, then stops `NEEDS_REVIEW` because the smoke semantic
  transport is not an independent judge;
- [ ] public `kill --escalate` removes the container and durable execution
  record, records `Cancelled`, and produces no marker, receipt or retry;
- [ ] worker `SIGKILL` removes the stale container before a replacement starts,
  schedules exactly one bounded retry, and reaches deterministic verification;
- [ ] `docker ps -a --filter label=io.deadreckon.managed=true` and the
  per-Job label query show no residual containers.

These tests prove the strict Docker lifecycle on the host and daemon used.
They do not prove Linux/bubblewrap parity, a real semantic `achieved`, a
machine reboot, or a signed cross-platform release. Record the command,
source revision and output before promoting the result from local evidence to
the clean-source dogfood record.

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
- [ ] before interruption, the protected sub-Plan has one ordered
  `sub_launch_prepared`, `sub_launched`, `sub_process_launch_prepared`,
  `sub_process_released`, and `sub_process_linked` authority chain;
- [ ] after lease reclaim, the replacement owner appends exactly one
  `sub_process_adopted` event with the same Job, sub, Plan, launch, PID, boot
  and process-start identities and the new lease epoch;
- [ ] the canonical adoption event is the protected intervention evidence and
  is followed by `sub_recovered` for the same sub-Plan;
- [ ] recovery appends no new logical or process launch fact and does not
  reopen a completed Plan task;
- [ ] the supervisor rebuilds the worst-of roll-up from current leaf evidence;
- [ ] a refused or changed roll-up stops before the semantic judge;
- [ ] semantic `revise` repairs only the merged Campaign parent and retains the
  completed sub-plan and leaf identities;
- [ ] a clean roll-up proceeds to parent gate, semantic judgment, receipt,
  promotion and `finish`;
- [ ] direct `campaign` creates a durable Campaign Job with the same roll-up
  and parent verification boundary.

## 8. Confirm continuation is a parent-bound Job

Use a disposable repository with a completed parent that has been promoted to
the library. Run both entry paths on separate parents:

```bash
"$WK_BIN" extend "$WK_PARENT_RUN_ID" \
  "Add one bounded follow-up change." \
  --provider "$WK_PROVIDER_A" \
  --max-spend 2 \
  --yes

"$WK_BIN" start "Add another bounded follow-up change."
```

For the second command, select the offered follow-up from completed history.
Record the returned child Job ID and inspect its launch plan, authority, status
and events.

Accept when:

- [ ] both public `extend` and guided `start` return one new Single Job ID and
  detach through the normal supervisor;
- [ ] the child source is the promoted parent artifact, not the operator's
  mutable checkout;
- [ ] the launch plan freezes the full parent run and scope, parent-state
  SHA-256, parent-library-tree SHA-256 and verified parent-receipt SHA-256 when
  the parent has one;
- [ ] the child records `durable_continuation_bound` before provider evidence;
- [ ] changing the frozen parent state, promoted artifact or receipt before
  child preparation is refused rather than silently continuing;
- [ ] `--dest` is refused at launch and the result can reach an operator
  destination only through `finish` after its own two-key receipt.

## 9. Exercise the user service and a real restart

This drill changes the per-user launchd/systemd definition. Do it on a
disposable user profile or only after recording the existing service posture:

```bash
"$WK_BIN" doctor --json
"$WK_BIN" supervisor status
"$WK_BIN" setup --supervisor
"$WK_BIN" supervisor status
"$WK_BIN" doctor
```

`setup --supervisor` is the normal one-step install-and-start path. The
lower-level `supervisor install` and `supervisor start` commands remain useful
when separately rehearsing those operator transitions.

Before repair, inspect `binary_health.installations` in the JSON and confirm
that the intended executable is marked `current`, the first shell-resolved copy
is marked `path-selected`, and any receipt or supervisor checkpoint roles name
the expected versions. If the running binary's own receipt is stale, run
`"$WK_BIN" doctor --repair`; it may back up and repoint the standard
shell-installer-owned executable selected by `PATH`, refresh the receipt, and
repair a managed service. It must leave Homebrew/npm/Cargo and arbitrary
user-owned binaries unchanged and report their channel-native update command.

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
- [ ] `doctor --json` identifies every reachable/known DeadReckon copy with its
  version and role; `doctor --repair` backs up and aliases only the standard
  active shell installation and does not overwrite package-manager/user copies;
- [ ] an unmanaged same-name definition is refused rather than overwritten;
- [ ] status reports a live schema-version-2 checkpoint whose boot ID, PID,
  process-start identity and instance belong to the current service;
- [ ] a real non-interactive `start --yes` refuses before provider work when
  the service is missing, stale or inactive;
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

Use a disposable Single Job whose worker route is a registry-backed HTTP
provider. Trusted prepare signs that exact worker role, provider route and
registry endpoint. It refuses CLI, local/loopback, caller-supplied and
unregistered endpoints.

Capture `network-reachable-before` while the protected helper can prove the
same supervised child before and after its bounded provider-registry ping.
Apply the reviewed host network change outside the recorder. While the exact
child is still current, record the intervention; this captures
`network-connectivity-observation` with `endpoint_unreachable`. Restore the
host change and capture `network-reachable-after` before cleanup. The recorder
does not persist the provider probe's free-form error message or credentials.

Accept when:

- [ ] the signed route and endpoint are observed reachable, unreachable and
  reachable again in that strict order;
- [ ] the before and unreachable probes bracket the same current process,
  launch, attempt and lease identity with one durable `child_linked` event;
- [ ] the exact affected attempt stops after the unreachable observation and
  is followed by its retry or a manifest-approved terminal result;
- [ ] `events-after`, `job-view-after` and the public report describe the same
  final append-only Job state;
- [ ] cleanup and pass both refuse a missing or unsuccessful restored probe;
- [ ] a verified result still has the independent deterministic and semantic
  completion receipt.

Network control is host-specific, so this checklist does not prescribe a
firewall command. The receipt proves the observed endpoint transition and
ordered Job response. It does not claim that a particular firewall command
caused the outage.

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
