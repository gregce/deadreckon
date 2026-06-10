#!/bin/sh
# Real-provider proof harness for a stable cut. Operator-run only: each
# route burns a few real provider turns (expect a small spend per route).
#
# Per route: a sandboxed DEADRECKON_HOME and a throwaway git repo, then
#   start -> run completes -> signed gate marker present -> apply succeeds
#   -> a second run is killed mid-turn and resumed to completion.
# On success the probed binary versions land in
# release/known-good-providers.json (schema_version 1).
#
# Usage: release/preflight-real.sh [route ...]
#        (defaults to cli:claude-code cli:codex)

set -eu

if [ -n "${CI:-}" ]; then
  echo "preflight-real.sh proves real providers and spends real money;" >&2
  echo "it refuses to run under CI (CI=${CI} is set). Run it yourself." >&2
  exit 1
fi

repo_root=$(cd "$(dirname "$0")/.." && pwd)
deadreckon_bin=${DEADRECKON_BIN:-"$repo_root/target/release/deadreckon"}
known_good="$repo_root/release/known-good-providers.json"
goal="add a comment header to README.md naming this file's purpose"
operator=${USER:-unknown}

if [ ! -x "$deadreckon_bin" ]; then
  echo "no release binary at $deadreckon_bin — run 'make build' first," >&2
  echo "or point DEADRECKON_BIN at one." >&2
  exit 1
fi

routes=${*:-"cli:claude-code cli:codex"}

workdir=$(mktemp -d "${TMPDIR:-/tmp}/deadreckon-preflight.XXXXXX")
trap 'rm -rf "$workdir"' EXIT INT TERM

binary_for_route() {
  case "$1" in
    cli:claude-code) echo "claude" ;;
    cli:codex) echo "codex" ;;
    cli:gemini) echo "gemini" ;;
    cli:copilot) echo "copilot" ;;
    *) echo "" ;;
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
    git add README.md
    git commit -qm "fixture"
  )
  echo "$dir"
}

latest_run_id() {
  # state.json lives at <home>/runstate/<task>/runs/<run_id>/state.json
  find "$1/runstate" -name state.json 2>/dev/null \
    | while IFS= read -r state; do
        dir=$(dirname "$state")
        printf '%s %s\n' "$(stat -f %m "$state" 2>/dev/null || stat -c %Y "$state")" "$(basename "$dir")"
      done \
    | sort -rn | head -n1 | cut -d' ' -f2
}

results=""

for route in $routes; do
  echo "==> proving $route"
  binary=$(binary_for_route "$route")
  if [ -z "$binary" ] || ! command -v "$binary" >/dev/null 2>&1; then
    echo "    SKIP: no installed binary for $route" >&2
    continue
  fi
  binary_version=$("$binary" --version 2>/dev/null | head -n1)

  home="$workdir/${route#cli:}-home"
  mkdir -p "$home"
  repo=$(fresh_repo "${route#cli:}")

  echo "    start: full run to completion"
  (
    cd "$repo"
    DEADRECKON_HOME="$home" "$deadreckon_bin" start "$goal" \
      --mode run --provider "$route" --yes --plain
  )

  run_id=$(latest_run_id "$home")
  [ -n "$run_id" ] || { echo "    FAIL: no run recorded for $route" >&2; exit 1; }

  marker=$(find "$home/runstate" -path "*/$run_id/*" -name turn-acceptance.json | head -n1)
  [ -n "$marker" ] || { echo "    FAIL: no turn-acceptance.json for $run_id" >&2; exit 1; }
  grep -q '"signature"' "$marker" \
    || { echo "    FAIL: gate marker for $run_id is unsigned" >&2; exit 1; }
  echo "    gate: signed acceptance marker present"

  (
    cd "$repo"
    DEADRECKON_HOME="$home" "$deadreckon_bin" apply "$run_id" --plain
  )
  echo "    apply: succeeded"

  echo "    kill/resume: interrupt a second run mid-turn"
  repo2=$(fresh_repo "${route#cli:}-resume")
  (
    cd "$repo2"
    DEADRECKON_HOME="$home" "$deadreckon_bin" start "$goal" \
      --mode run --provider "$route" --yes --plain
  ) &
  bg_pid=$!
  sleep 20
  resume_id=$(latest_run_id "$home")
  DEADRECKON_HOME="$home" "$deadreckon_bin" kill "$resume_id" --plain || true
  wait "$bg_pid" 2>/dev/null || true
  (
    cd "$repo2"
    DEADRECKON_HOME="$home" "$deadreckon_bin" resume "$resume_id" --plain
  )
  echo "    kill/resume: resumed to completion"

  entry=$(printf '{"route":"%s","binary_version":"%s","proof":"start -> real turns -> gate signed -> apply -> kill/resume","run_id":"%s","operator":"%s"}' \
    "$route" "$binary_version" "$run_id" "$operator")
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
cat > "$known_good" <<EOF
{
  "schema_version": 1,
  "recorded_at": "$recorded_at",
  "providers": [
    $results
  ]
}
EOF
echo "==> wrote $known_good"
