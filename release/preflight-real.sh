#!/bin/sh
# Real-provider proof harness for a stable cut. Operator-run only: each
# route consumes a few real provider turns (subscription quota for CLI routes).
#
# Per route: a sandboxed DEADRECKON_HOME and a throwaway git repo, then
#   durable run -> verified receipt -> signed gate marker -> finish succeeds
#   -> a second run is cancelled mid-turn and its provider tree is reaped.
# On success the probed binary versions land in
# release/known-good-providers.json (schema_version 3), bound to the exact
# source-derived build bundle and all three shipped binary digests.
# Worker and semantic-review routes are recorded separately: generic CLI
# workers use a schema-capable reviewer instead of being credited with a
# judgment their adapter cannot make.
#
# Usage: release/preflight-real.sh [route ...]
#        (defaults to cli:claude-code cli:codex)

set -eu

if [ -n "${CI:-}" ]; then
  echo "preflight-real.sh proves real providers and consumes real provider quota;" >&2
  echo "it refuses to run under CI (CI=${CI} is set). Run it yourself." >&2
  exit 1
fi

repo_root=$(cd "$(dirname "$0")/.." && pwd)
deadreckon_bin=${DEADRECKON_BIN:-"$repo_root/target/release/deadreckon"}
known_good=${PREFLIGHT_KNOWN_GOOD:-"$repo_root/release/known-good-providers.json"}
release_trust="$repo_root/release/trust/release-trust.mjs"
goal="make purpose.sh print exactly: DeadReckon provider preflight fixture"
operator=${USER:-unknown}
job_timeout_seconds=${PREFLIGHT_JOB_TIMEOUT_SECONDS:-900}

if [ ! -x "$deadreckon_bin" ]; then
  echo "no release binary at $deadreckon_bin — run 'make build' first," >&2
  echo "or point DEADRECKON_BIN at one." >&2
  exit 1
fi
if ! command -v node >/dev/null 2>&1; then
  echo "node is required to bind provider evidence to the release bundle" >&2
  exit 1
fi

# The build identity is calculated from these exact tracked inputs. Refuse a
# proof whose source commit cannot reproduce the working-tree inputs used by
# the binary. Unrelated operator notes and generated target directories remain
# outside this boundary.
if ! git -C "$repo_root" diff --quiet -- Cargo.toml Cargo.lock crates \
  || ! git -C "$repo_root" diff --cached --quiet -- Cargo.toml Cargo.lock crates; then
  echo "release Rust/Cargo inputs are dirty — commit them, rebuild, then rerun preflight" >&2
  exit 1
fi
untracked_inputs=$(git -C "$repo_root" ls-files --others --exclude-standard -- \
  Cargo.toml Cargo.lock ':(glob)crates/*/Cargo.toml' ':(glob)crates/*/build.rs' \
  ':(glob)crates/*/src/**/*.rs')
if [ -n "$untracked_inputs" ]; then
  echo "release build inputs are untracked — commit or remove them before preflight:" >&2
  printf '%s\n' "$untracked_inputs" >&2
  exit 1
fi

routes=${*:-"cli:claude-code cli:codex"}

workdir=$(mktemp -d "${TMPDIR:-/tmp}/deadreckon-preflight.XXXXXX")
# On success the workdir is noise; on failure it is the only evidence of WHY
# the proof failed -- the run state, the agent's tree, and the gate's output.
# Deleting it unconditionally made a failed preflight undiagnosable after the
# fact and forced a second real-provider run just to see the error.
cleanup_workdir() {
  status=$?
  if [ "$status" -eq 0 ]; then
    rm -rf "$workdir"
  else
    echo "preflight failed (exit $status) — evidence preserved at $workdir" >&2
  fi
}
trap cleanup_workdir EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

bundle_proof="$workdir/release-bundle-proof.json"
node "$release_trust" bundle-proof \
  --root "$repo_root" \
  --deadreckon "$deadreckon_bin" \
  --out "$bundle_proof" >/dev/null
source_commit=$(git -C "$repo_root" rev-parse HEAD)

binary_for_route() {
  case "$1" in
    cli:claude-code) echo "claude" ;;
    cli:codex) echo "codex" ;;
    cli:codex-server) echo "codex" ;;
    cli:gemini) echo "gemini" ;;
    cli:opencode) echo "opencode" ;;
    cli:copilot) echo "copilot" ;;
    cli:pi) echo "pi" ;;
    *) echo "" ;;
  esac
}

reviewer_for_route() {
  if [ -n "${PREFLIGHT_REVIEWER_PROVIDER:-}" ]; then
    echo "$PREFLIGHT_REVIEWER_PROVIDER"
    return
  fi
  case "$1" in
    cli:gemini|cli:opencode|cli:copilot|cli:pi) echo "cli:codex" ;;
    *) echo "$1" ;;
  esac
}

fresh_repo() {
  dir="$workdir/$1-repo"
  rm -rf "$dir"
  mkdir -p "$dir"
  (
    cd "$dir"
    git init -q
    git config user.email preflight@deadreckon.local
    git config user.name "deadreckon preflight"
    printf '# preflight fixture\n' > README.md
    printf '#!/bin/sh\nprintf "%%s\\n" "unfinished preflight fixture"\n' > purpose.sh
    chmod +x purpose.sh
    git add README.md purpose.sh
    git commit -qm "fixture"
  )
  echo "$dir"
}

write_fixture_contract() {
  dir=$1
  mkdir -p "$dir/.deadreckon"
  cat > "$dir/.deadreckon/acceptance.yaml" <<'EOF'
name: real provider preflight
checks:
  - kind: shell
    command: >-
      output=$(sh purpose.sh); test "$output" = "DeadReckon provider preflight fixture"
    cwd: "{working_dir}"
EOF
  cat > "$dir/.deadreckon/acceptance.md" <<'EOF'
# Real provider preflight done criteria

Running `purpose.sh` must print exactly:

`DeadReckon provider preflight fixture`

The executable check runs the delivered script and compares its observed output
with that exact expected value.
EOF
}

commit_fixture_contract() {
  repo=$1
  home=$2
  write_fixture_contract "$repo"
  (
    cd "$repo"
    DEADRECKON_HOME="$home" "$deadreckon_bin" def-done show >/dev/null
    if DEADRECKON_HOME="$home" "$deadreckon_bin" def-done check >/dev/null 2>&1; then
      echo "    FAIL: preflight contract passed before provider work" >&2
      exit 1
    fi
    git add -A
    git commit -qm "done criteria"
  )
}

latest_job_id() {
  find "$1/jobs" -name job.json 2>/dev/null \
    | while IFS= read -r job; do
        printf '%s %s\n' "$(stat -f %m "$job" 2>/dev/null || stat -c %Y "$job")" "$(basename "$(dirname "$job")")"
      done \
    | sort -rn | head -n1 | cut -d' ' -f2
}

wait_for_verified_job() {
  home=$1
  job_id=$2
  deadline=$(( $(date +%s) + job_timeout_seconds ))
  projection="$home/jobs/$job_id/projection.json"
  while [ "$(date +%s)" -lt "$deadline" ]; do
    phase=$(sed -n 's/^[[:space:]]*"phase": "\([^"]*\)".*$/\1/p' "$projection" 2>/dev/null | head -n1)
    if [ "$phase" = "terminal" ]; then
      outcome=$(sed -n 's/^[[:space:]]*"outcome": "\([^"]*\)".*$/\1/p' "$projection" | head -n1)
      if [ "$outcome" = "verified" ]; then
        return 0
      fi
      echo "    FAIL: job $job_id ended with outcome ${outcome:-unknown}" >&2
      DEADRECKON_HOME="$home" "$deadreckon_bin" status "$job_id" --plain >&2 || true
      return 1
    fi
    sleep 2
  done
  echo "    FAIL: job $job_id did not finish within ${job_timeout_seconds}s" >&2
  DEADRECKON_HOME="$home" "$deadreckon_bin" status "$job_id" --plain >&2 || true
  return 1
}

launch_route_job() {
  repo=$1
  home=$2
  route=$3
  reviewer=$4
  (
    cd "$repo"
    DEADRECKON_HOME="$home" "$deadreckon_bin" run "$goal" \
      --provider "$route" --reviewer-provider "$reviewer" \
      --yes --quiet --plain --no-docs >&2
  )
  latest_job_id "$home"
}

wait_for_provider_pid() {
  home=$1
  run_id=$2
  deadline=$(( $(date +%s) + 60 ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    pid_file=$(find "$home/runstate" -path "*/runs/$run_id/child-pids/provider-turn-*.pid" \
      -type f 2>/dev/null | sort | tail -n1)
    if [ -n "$pid_file" ]; then
      pid=$(sed -n 's/.*"pid":[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$pid_file" | head -n1)
      if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        printf '%s\n' "$pid"
        return 0
      fi
    fi
    sleep 1
  done
  echo "    FAIL: no live identity-bound provider process appeared for run $run_id" >&2
  return 1
}

assert_job_cancelled() {
  home=$1
  job_id=$2
  projection="$home/jobs/$job_id/projection.json"
  deadline=$(( $(date +%s) + 30 ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    phase=$(sed -n 's/^[[:space:]]*"phase": "\([^"]*\)".*$/\1/p' "$projection" | head -n1)
    outcome=$(sed -n 's/^[[:space:]]*"outcome": "\([^"]*\)".*$/\1/p' "$projection" | head -n1)
    if [ "$phase" = "terminal" ] && [ "$outcome" = "cancelled" ]; then
      return 0
    fi
    sleep 1
  done
  echo "    FAIL: cancelled Job $job_id recorded ${phase:-unknown}/${outcome:-unknown}" >&2
  DEADRECKON_HOME="$home" "$deadreckon_bin" status "$job_id" --plain >&2 || true
  return 1
}

assert_process_reaped() {
  pid=$1
  deadline=$(( $(date +%s) + 10 ))
  while kill -0 "$pid" 2>/dev/null && [ "$(date +%s)" -lt "$deadline" ]; do
    sleep 1
  done
  if kill -0 "$pid" 2>/dev/null; then
    echo "    FAIL: cancelled provider process $pid is still alive" >&2
    return 1
  fi
}

results=""

for route in $routes; do
  echo "==> proving $route"
  binary=$(binary_for_route "$route")
  if [ -z "$binary" ] || ! command -v "$binary" >/dev/null 2>&1; then
    echo "    FAIL: no installed binary for requested route $route" >&2
    exit 1
  fi
  binary_version=$("$binary" --version 2>/dev/null | head -n1)
  reviewer=$(reviewer_for_route "$route")
  reviewer_binary=$(binary_for_route "$reviewer")
  if [ -z "$reviewer_binary" ] || ! command -v "$reviewer_binary" >/dev/null 2>&1; then
    echo "    FAIL: no installed binary for semantic reviewer route $reviewer" >&2
    exit 1
  fi
  reviewer_binary_version=$("$reviewer_binary" --version 2>/dev/null | head -n1)
  echo "    routes: worker=$route semantic-reviewer=$reviewer"

  home="$workdir/${route#cli:}-home"
  mkdir -p "$home"
  repo=$(fresh_repo "${route#cli:}")

  echo "    run: durable execution to completion"
  commit_fixture_contract "$repo" "$home"
  job_id=$(launch_route_job "$repo" "$home" "$route" "$reviewer")
  [ -n "$job_id" ] || { echo "    FAIL: no Job recorded for $route" >&2; exit 1; }
  wait_for_verified_job "$home" "$job_id"

  # A durable single-leaf Job deliberately reuses its Job id for the child
  # Run. Avoid a newest-by-mtime guess once this isolated home contains the
  # second cancellation fixture.
  run_id=$job_id

  marker=$(find "$home/runstate" -path "*/$run_id/*" -name turn-acceptance.json | head -n1)
  [ -n "$marker" ] || { echo "    FAIL: no turn-acceptance.json for $run_id" >&2; exit 1; }
  grep -q '"signature"' "$marker" \
    || { echo "    FAIL: gate marker for $run_id is unsigned" >&2; exit 1; }
  echo "    gate: signed acceptance marker present"

  (
    cd "$repo"
    DEADRECKON_HOME="$home" "$deadreckon_bin" finish "$job_id" --no-confirm --cleanup
  )
  [ "$(sh "$repo/purpose.sh")" = "DeadReckon provider preflight fixture" ] \
    || { echo "    FAIL: finished result was not delivered" >&2; exit 1; }
  echo "    finish: verified receipt delivered"

  echo "    cancel: interrupt a second run and reap its provider tree"
  repo2=$(fresh_repo "${route#cli:}-cancel")
  commit_fixture_contract "$repo2" "$home"
  cancel_job_id=$(launch_route_job "$repo2" "$home" "$route" "$reviewer")
  [ -n "$cancel_job_id" ] || { echo "    FAIL: no cancellation Job recorded for $route" >&2; exit 1; }
  cancel_run_id=$cancel_job_id
  provider_pid=$(wait_for_provider_pid "$home" "$cancel_run_id")
  DEADRECKON_HOME="$home" "$deadreckon_bin" kill "$cancel_job_id" --plain --escalate
  assert_job_cancelled "$home" "$cancel_job_id"
  assert_process_reaped "$provider_pid"
  echo "    cancel: Job terminal and provider process reaped"

  entry=$(printf '{"route":"%s","binary_version":"%s","reviewer_route":"%s","reviewer_binary_version":"%s","proof":"worker turn -> deterministic gate -> separate schema-only semantic judgment -> verified receipt -> gate signed -> finish -> cancel/reap","run_id":"%s","operator":"%s"}' \
    "$route" "$binary_version" "$reviewer" "$reviewer_binary_version" "$run_id" "$operator")
  if [ -n "$results" ]; then
    results="$results,
    $entry"
  else
    results="$entry"
  fi
done

if [ -z "$results" ]; then
  echo "no routes proved — nothing written to $known_good" >&2
  exit 1
fi

recorded_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
# The exact version and bundle actually proved. On failure this file remains
# untouched, and stable validation rejects either an old schema or a source ID
# that does not match the tag checkout.
proved_version=$("$deadreckon_bin" --version 2>/dev/null | awk '{print $2}')
bundle_version=$(sed -n 's/^[[:space:]]*"package_version": "\([^"]*\)".*$/\1/p' "$bundle_proof" | head -n1)
if [ -z "$proved_version" ] || [ "$proved_version" != "$bundle_version" ]; then
  echo "proved binary version ${proved_version:-unknown} does not match bundle source version ${bundle_version:-unknown}" >&2
  exit 1
fi
cat > "$known_good" <<EOF
{
  "schema_version": 3,
  "recorded_at": "$recorded_at",
  "source_commit": "$source_commit",
  "deadreckon_version": "$proved_version",
  "bundle": $(cat "$bundle_proof"),
  "providers": [
    $results
  ]
}
EOF
required_routes=$(printf '%s' "$routes" | tr ' ' ',')
node "$release_trust" verify-provider-proof \
  --root "$repo_root" \
  --version "$proved_version" \
  --known-good "$known_good" \
  --required-routes "$required_routes" >/dev/null
echo "==> wrote $known_good"
