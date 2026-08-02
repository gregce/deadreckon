# Soundings operator acceptance

Use this checklist from the operator's seat after building the candidate. It
tests the source decision, approved Graph copy, bounded provider cleanup and
contract reuse. It does not require a paid provider for the timeout/retry
checks.

## 1. Build the binary you intend to test

Working directory: `/Users/gdc/deadreckon`.

```sh
cargo build --release -p deadreckon
./target/release/deadreckon --version
./target/release/deadreckon doctor
```

Expected signal: the version is the repository candidate and `doctor` reports
no blocking setup failure. If `doctor` reports another PATH-selected binary,
continue with `./target/release/deadreckon` so this checklist does not test an
older installation by accident.

## 2. Preview the exact Flappy continuation source

Choose the launch project that should own the new done contract. The example
uses the existing test project:

```sh
cd /Users/gdc/test-deadreckon/test-flappy
SOURCE=/Users/gdc/.deadreckon/worktrees/task-0-00ba509b-ea6701b0
DR=/Users/gdc/deadreckon/target/release/deadreckon
test -d "$SOURCE"

"$DR" start \
  "Continue the existing native macOS Flappy Bird app. Add inventive visual and gameplay customization, persist the settings, verify gameplay, and complete a final correctness and polish review." \
  --mode full-plan \
  --from "$SOURCE" \
  --preview \
  --plain
```

Expected signals:

- the preview says `copy from` or `approved copy from` followed by the
  canonical source path;
- when authoring is needed, it names the source as the inspection root, this
  launch project as the writer root, the structured provider/model route and a
  `120s` total limit;
- it does not say that `--from` is unsupported;
- preview creates no Job.

## 3. Accept the contract and create the Graph Job

Record a deliverable fingerprint before launch, then run the exact command:

```sh
SOURCE_FINGERPRINT_BEFORE=$(
  cd "$SOURCE" &&
  git ls-files -co --exclude-standard -z |
  xargs -0 shasum -a 256 |
  shasum -a 256
)

"$DR" start \
  "Continue the existing native macOS Flappy Bird app. Add inventive visual and gameplay customization, persist the settings, verify gameplay, and complete a final correctness and polish review." \
  --mode full-plan \
  --from "$SOURCE" \
  --review-done
```

Review the generated checks and accept only if they describe the real app.

Expected signals:

- the contract review names `Cloudwing`, its Swift source/tests, or equivalent
  facts that really exist in the source; it does not invent a `FlappyBird`
  package or target;
- after acceptance, the command returns one Job ID and prints `attach`,
  `status`, `kill` and `finish` actions;
- there is no late `--from` refusal.

Set `JOB_ID` to the returned full ID and inspect it:

```sh
JOB_ID=<returned-job-id>
"$DR" show "$JOB_ID" --plain
jq -r '.shape, .source_cwd' "$HOME/.deadreckon/jobs/$JOB_ID/job.json"
jq -r '.signals.watchkeeper_source, .signals.watchkeeper_driver' \
  "$HOME/.deadreckon/jobs/$JOB_ID/launch-plan.json"
```

Expected signals:

- shape is `graph`;
- `source_cwd` is below
  `$HOME/.deadreckon/jobs/$JOB_ID/approved-source`, not the external source;
- launch-plan source mode is `copy`, `from` is the canonical external path,
  and the driver has `source_init_git: true`.

Confirm the external source is byte-identical for tracked and untracked
deliverables:

```sh
SOURCE_FINGERPRINT_AFTER=$(
  cd "$SOURCE" &&
  git ls-files -co --exclude-standard -z |
  xargs -0 shasum -a 256 |
  shasum -a 256
)
test "$SOURCE_FINGERPRINT_BEFORE" = "$SOURCE_FINGERPRINT_AFTER"
```

Expected signal: `test` exits 0.

## 4. Prove a hanging authoring provider is bounded and reaped

Working directory: `/Users/gdc/deadreckon`.

```sh
cargo test -j 1 -p deadreckon-providers --lib \
  cli_common::tests::done_timeout_reaps_provider_and_grandchild_processes \
  -- --exact --nocapture

cargo test -j 1 -p deadreckon --bin deadreckon \
  commands::acceptance::tests::done_authoring_latency_matrix_enforces_120_second_default \
  -- --exact --nocapture
```

Expected signals: both commands report `1 passed; 0 failed`. The first starts a
provider plus grandchild, cancels it, waits for both to disappear and removes
the PID file. The second pins the 120-second cumulative default, 60-second
draft ceiling, 20-second critic ceiling and remaining-time-only redraft.

## 5. Prove retry reuses a valid written contract

Working directory: `/Users/gdc/deadreckon`.

```sh
cargo test -j 1 -p deadreckon --features internal-characterization \
  --test orchestrate retry_reuses_valid_generated_contract_without_provider_call \
  -- --exact --nocapture
```

Expected signal: `1 passed; 0 failed`. The scripted provider would leave a call
marker if invoked; the retry discovers the valid contract on disk and creates
no marker.

## 6. Run the complete hermetic reproduction

```sh
cargo test -j 1 -p deadreckon --features internal-characterization \
  --test orchestrate flappy_reproduction_returns_graph_job_id_after_accepting_done_contract \
  -- --exact --nocapture
```

Expected signal: `1 passed; 0 failed`. This covers empty launch directory,
dirty and untracked Cloudwing inputs, preview, accepted contract, returned Job
ID, approved copy, matching authority/driver source truth and unchanged
operator source.

## Not covered by this checklist

- live-provider response quality or a claim that real authoring normally takes
  2.76 seconds; the measured run is hermetic correctness evidence;
- Campaign `--from` or remote/cross-machine source transport;
- real machine reboot recovery, live provider-network interruption, notarized
  release installation, or false-acceptance/false-rejection rates;
- final completion of the Flappy app. This checklist proves admission and
  durable launch behavior, not that the later worker has earned its two-key
  receipt.
