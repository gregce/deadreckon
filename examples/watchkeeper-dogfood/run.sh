#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
matrix_path="${DEADRECKON_DOGFOOD_MATRIX:-$script_dir/matrix.json}"
expected_matrix_sha256="${DEADRECKON_DOGFOOD_MATRIX_SHA256:-}"
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
  python3 - "$script_dir" "$matrix_path" "$task_id" "$artifact_root" "$expected_matrix_sha256" <<'PY'
import sys
from pathlib import Path

script_dir, matrix_path, task_id, artifact_root, expected_matrix_sha256 = sys.argv[1:]
sys.path.insert(0, script_dir)
from dogfood_common import load_matrix, validated_task_artifact_root

manifest = load_matrix(Path(matrix_path))
if expected_matrix_sha256 and manifest.sha256 != expected_matrix_sha256:
    raise SystemExit(
        "refusing dogfood execution because matrix bytes changed after batch planning: "
        f"expected {expected_matrix_sha256}, observed {manifest.sha256}"
    )
tasks = manifest.tasks_by_id
if task_id not in tasks:
    raise SystemExit(f"unknown dogfood task: {task_id}")
task = tasks[task_id]
task_root = validated_task_artifact_root(Path(artifact_root), task_id)
if task_root.is_dir() and any(task_root.iterdir()):
    raise SystemExit(
        f"refusing to start a duplicate Job for {task_id}: {task_root} is non-empty; "
        "inspect the recorded Job, then archive or repair these artifacts"
    )
fields = [
    manifest.repository_path_env[task.repository_slot],
    manifest.provider_route_env[task.provider_slot],
    task.repository_slot,
    task.provider_slot,
    manifest.sha256,
    task.goal,
    str(task.max_spend_usd),
]
if any("\t" in field or "\n" in field for field in fields):
    raise SystemExit("matrix fields used by the harness must be one-line tab-free strings")
print("\t".join(fields))
PY
)"
IFS=$'\t' read -r repository_env provider_env repository_slot provider_slot matrix_sha256 goal max_spend_usd <<<"$task_row"
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

mkdir -p "$artifact_root"
if ! mkdir "$artifact_root/$task_id"; then
  echo "refusing to start a duplicate Job for $task_id: the task artifact directory already exists; inspect the recorded Job, then archive or repair these artifacts" >&2
  exit 65
fi
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
  python3 - "$script_dir" "$start_tmp" <<'PY'
import json
import sys

script_dir, start_path = sys.argv[1:]
sys.path.insert(0, script_dir)
from dogfood_common import validated_path_id

with open(start_path, encoding="utf-8") as handle:
    payload = json.load(handle)
ids = payload.get("dispatched", {}).get("ids", [])
if not isinstance(ids, list) or len(ids) != 1:
    raise SystemExit("public start output must contain exactly one dispatched job id")
print(validated_path_id(ids[0], "public start job id"))
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

python3 - "$observation_dir/operator-run.json" "$observation_dir/job-view.json" "$task_id" "$job_id" "$repository_slot" "$provider_slot" "$matrix_sha256" "$repository" "$provider" <<'PY'
import datetime
import json
import sys

(
    output,
    job_view_path,
    task_id,
    job_id,
    repository_slot,
    provider_slot,
    matrix_sha256,
    repository,
    provider,
) = sys.argv[1:]
with open(job_view_path, encoding="utf-8") as handle:
    job_view = json.load(handle)
projection = job_view["job"]["projection"]
record = {
    "schema_version": 1,
    "task_id": task_id,
    "job_id": job_id,
    "matrix_sha256": matrix_sha256,
    "repository_slot": repository_slot,
    "provider_slot": provider_slot,
    "repository": repository,
    "provider_slot_value": provider,
    "terminal_observed_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "terminal_outcome": projection.get("outcome"),
    "terminal_stop_reason": projection.get("stop_reason"),
    "public_commands": ["start", "status"],
    "report_attempted": False,
    "report_succeeded": False,
    "receipt_validation_attempted": False,
    "receipt_validated": False,
    "finish_attempted": False,
}
with open(output, "w", encoding="utf-8") as handle:
    json.dump(record, handle, indent=2, sort_keys=True)
    handle.write("\n")
PY

report_path="$observation_dir/job-report.json"
set +e
(
  cd "$repository"
  "$deadreckon_bin" report "$job_id" --plain --json
) >"$report_path" 2>"$observation_dir/job-report.err"
report_status=$?
set -e

report_validation_status=$report_status
if (( report_status == 0 )); then
  set +e
  python3 - "$report_path" "$job_id" <<'PY'
import json
import sys

report_path, job_id = sys.argv[1:]
with open(report_path, encoding="utf-8") as handle:
    report = json.load(handle)
required_report = {
    "id": job_id,
    "phase": "terminal",
    "outcome": "verified",
    "stop_reason": "verified",
}
for field, expected in required_report.items():
    if report.get(field) != expected:
        raise SystemExit(
            f"report field {field} was {report.get(field)!r}, expected {expected!r}"
        )

receipt_report = report.get("receipt")
if not isinstance(receipt_report, dict):
    raise SystemExit("public report omitted receipt validation")
if receipt_report.get("status") != "valid":
    raise SystemExit(
        f"public report classified the completion receipt as {receipt_report.get('status')!r}"
    )
if receipt_report.get("signature_validation_error") is not None:
    raise SystemExit(
        f"public report included a receipt validation error: "
        f"{receipt_report.get('signature_validation_error')}"
    )
if receipt_report.get("sandbox_backend") in (None, "none"):
    raise SystemExit("validated receipt does not prove a sandbox backend")

validated_receipt = receipt_report.get("receipt")
if not isinstance(validated_receipt, dict):
    raise SystemExit("public report omitted the validated receipt")
required_receipt = {
    "job_id": job_id,
    "proof_kind": "two_key_completion",
    "contained": True,
}
for field, expected in required_receipt.items():
    if validated_receipt.get(field) != expected:
        raise SystemExit(
            f"validated receipt field {field} was "
            f"{validated_receipt.get(field)!r}, expected {expected!r}"
        )
PY
  report_validation_status=$?
  set -e
fi

python3 - "$observation_dir/operator-run.json" "$report_status" "$report_validation_status" <<'PY'
import json
import sys

path, report_status, validation_status = sys.argv[1:]
with open(path, encoding="utf-8") as handle:
    record = json.load(handle)
record["public_commands"] = ["start", "status", "report"]
record["report_attempted"] = True
record["report_exit_status"] = int(report_status)
record["report_succeeded"] = int(report_status) == 0
record["receipt_validation_attempted"] = True
record["receipt_validation_source"] = "deadreckon report --json"
record["receipt_validation_exit_status"] = int(validation_status)
record["receipt_validated"] = int(validation_status) == 0
with open(path, "w", encoding="utf-8") as handle:
    json.dump(record, handle, indent=2, sort_keys=True)
    handle.write("\n")
PY
if (( report_validation_status != 0 )); then
  exit "$report_validation_status"
fi

receipt_path="$DEADRECKON_HOME/jobs/$job_id/receipt.json"
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
record["public_commands"] = ["start", "status", "report", "finish"]
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
