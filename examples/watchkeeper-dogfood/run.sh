#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
matrix_path="${DEADRECKON_DOGFOOD_MATRIX:-$script_dir/matrix.json}"
task_id="${1:-}"

if [[ "${DEADRECKON_DOGFOOD_EXECUTE:-}" != "1" ]]; then
  echo "refusing to run providers: set DEADRECKON_DOGFOOD_EXECUTE=1 after reviewing the matrix" >&2
  exit 64
fi
if [[ -z "$task_id" ]]; then
  echo "usage: DEADRECKON_DOGFOOD_EXECUTE=1 $0 TASK_ID" >&2
  exit 64
fi
if [[ -z "${DEADRECKON_HOME:-}" ]]; then
  echo "DEADRECKON_HOME must point at an isolated dogfood state directory" >&2
  exit 64
fi

deadreckon_bin="${DEADRECKON_BIN:-deadreckon}"
poll_seconds="${DEADRECKON_DOGFOOD_POLL_SECONDS:-5}"
max_polls="${DEADRECKON_DOGFOOD_MAX_POLLS:-720}"
artifact_root="${DEADRECKON_DOGFOOD_ARTIFACTS:-$script_dir/artifacts}"

task_row="$(
  python3 - "$matrix_path" "$task_id" <<'PY'
import json
import sys

matrix_path, task_id = sys.argv[1:]
with open(matrix_path, encoding="utf-8") as handle:
    matrix = json.load(handle)
tasks = {task["id"]: task for task in matrix["tasks"]}
if task_id not in tasks:
    raise SystemExit(f"unknown dogfood task: {task_id}")
task = tasks[task_id]
repositories = {entry["slot"]: entry for entry in matrix["repositories"]}
providers = {entry["slot"]: entry for entry in matrix["providers"]}
fields = [
    repositories[task["repository"]]["path_env"],
    providers[task["provider"]]["route_env"],
    task["goal"],
    str(task["max_spend_usd"]),
]
if any("\t" in field or "\n" in field for field in fields):
    raise SystemExit("matrix fields used by the harness must be one-line tab-free strings")
print("\t".join(fields))
PY
)"
IFS=$'\t' read -r repository_env provider_env goal max_spend_usd <<<"$task_row"
repository="${!repository_env:-}"
provider="${!provider_env:-}"

if [[ -z "$repository" || ! -d "$repository" ]]; then
  echo "$repository_env must name an existing disposable repository" >&2
  exit 64
fi
if [[ -z "$provider" ]]; then
  echo "$provider_env must name a configured provider route" >&2
  exit 64
fi

mkdir -p "$artifact_root/$task_id"
start_tmp="$artifact_root/$task_id/start.tmp.json"

(
  cd "$repository"
  "$deadreckon_bin" start "$goal" \
    --mode run \
    --provider "$provider" \
    --max-spend "$max_spend_usd" \
    --yes \
    --quiet \
    --plain \
    --json
) >"$start_tmp"

job_id="$(
  python3 - "$start_tmp" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    payload = json.load(handle)
ids = payload.get("dispatched", {}).get("ids", [])
if not ids:
    raise SystemExit("public start output did not contain a dispatched job id")
print(ids[0])
PY
)"
observation_dir="$artifact_root/$task_id/$job_id"
mkdir -p "$observation_dir"
mv "$start_tmp" "$observation_dir/start.json"

poll=0
while :; do
  poll=$((poll + 1))
  (
    cd "$repository"
    "$deadreckon_bin" status "$job_id" --plain --json
  ) >"$observation_dir/status-latest.json"
  phase="$(
    python3 - "$observation_dir/status-latest.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    payload = json.load(handle)
print(payload["job"]["projection"]["phase"])
PY
  )"
  if [[ "$phase" == "terminal" ]]; then
    cp "$observation_dir/status-latest.json" "$observation_dir/job-view.json"
    break
  fi
  if (( poll >= max_polls )); then
    echo "job $job_id did not become terminal after $max_polls status polls" >&2
    exit 70
  fi
  sleep "$poll_seconds"
done

python3 - "$observation_dir/operator-run.json" "$observation_dir/job-view.json" "$task_id" "$job_id" "$repository" "$provider" <<'PY'
import datetime
import json
import sys

output, job_view_path, task_id, job_id, repository, provider = sys.argv[1:]
with open(job_view_path, encoding="utf-8") as handle:
    job_view = json.load(handle)
projection = job_view["job"]["projection"]
record = {
    "schema_version": 1,
    "task_id": task_id,
    "job_id": job_id,
    "repository": repository,
    "provider_slot_value": provider,
    "terminal_observed_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "terminal_outcome": projection.get("outcome"),
    "terminal_stop_reason": projection.get("stop_reason"),
    "public_commands": ["start", "status"],
    "receipt_validation_attempted": False,
    "receipt_validated": False,
    "finish_attempted": False,
}
with open(output, "w", encoding="utf-8") as handle:
    json.dump(record, handle, indent=2, sort_keys=True)
    handle.write("\n")
PY

receipt_path="$DEADRECKON_HOME/jobs/$job_id/receipt.json"
set +e
python3 - "$receipt_path" "$job_id" <<'PY'
import json
import os
import sys

receipt_path, job_id = sys.argv[1:]
if not os.path.isfile(receipt_path):
    raise SystemExit(f"verified completion receipt is missing: {receipt_path}")
with open(receipt_path, encoding="utf-8") as handle:
    receipt = json.load(handle)
required = {
    "job_id": job_id,
    "run_id": job_id,
    "outcome": "verified",
    "stop_reason": "verified",
    "issuer": "deadreckon-supervisor",
    "proof_kind": "two_key_completion",
    "contained": True,
}
for field, expected in required.items():
    if receipt.get(field) != expected:
        raise SystemExit(
            f"receipt field {field} was {receipt.get(field)!r}, expected {expected!r}"
        )
for digest in (
    "authority_sha256",
    "contract_sha256",
    "deterministic_marker_sha256",
    "semantic_judgment_sha256",
    "result_tree_sha256",
    "signature",
):
    if not receipt.get(digest):
        raise SystemExit(f"receipt omitted {digest}")
PY
receipt_validation_status=$?
set -e

python3 - "$observation_dir/operator-run.json" "$receipt_validation_status" <<'PY'
import json
import sys

path, status = sys.argv[1:]
with open(path, encoding="utf-8") as handle:
    record = json.load(handle)
record["receipt_validation_attempted"] = True
record["receipt_validation_exit_status"] = int(status)
record["receipt_validated"] = int(status) == 0
with open(path, "w", encoding="utf-8") as handle:
    json.dump(record, handle, indent=2, sort_keys=True)
    handle.write("\n")
PY
if (( receipt_validation_status != 0 )); then
  exit "$receipt_validation_status"
fi

cp "$receipt_path" "$observation_dir/receipt.json"

set +e
(
  cd "$repository"
  "$deadreckon_bin" finish "$job_id" --no-confirm
) >"$observation_dir/finish.out" 2>"$observation_dir/finish.err"
finish_status=$?
set -e

python3 - "$observation_dir/operator-run.json" "$finish_status" <<'PY'
import datetime
import json
import sys

output, status = sys.argv[1:]
with open(output, encoding="utf-8") as handle:
    record = json.load(handle)
record["completed_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
record["public_commands"] = ["start", "status", "finish"]
record["finish_attempted"] = True
record["finish_exit_status"] = int(status)
record["finish_succeeded"] = int(status) == 0
with open(output, "w", encoding="utf-8") as handle:
    json.dump(record, handle, indent=2, sort_keys=True)
    handle.write("\n")
PY
if (( finish_status != 0 )); then
  exit "$finish_status"
fi

echo "$observation_dir"
