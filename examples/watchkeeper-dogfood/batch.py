#!/usr/bin/env python3
"""Plan or explicitly execute the matrix as a resumable operator campaign."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
from typing import Any

from dogfood_common import (
    load_matrix,
    valid_terminal_observations,
    validated_task_artifact_root,
)


def build_plan(matrix_path: Path, artifacts: Path) -> dict[str, Any]:
    manifest = load_matrix(matrix_path)
    rows: list[dict[str, Any]] = []
    for task in manifest.tasks:
        task_root = validated_task_artifact_root(artifacts, task.task_id)
        valid, invalid = valid_terminal_observations(
            task_root, task, manifest.sha256
        )
        if len(valid) > 1:
            raise ValueError(
                f"duplicate terminal observations for matrix task {task.task_id}: "
                + ", ".join(observation.job_id for observation in valid)
            )
        entries = sorted(task_root.iterdir()) if task_root.is_dir() else []
        valid_entry = valid[0].directory if valid else None
        ambiguous_entries = [entry for entry in entries if entry != valid_entry]
        if valid and not ambiguous_entries:
            action = "skip_terminal"
        elif invalid:
            action = "blocked_invalid"
        elif entries:
            action = "blocked_partial"
        else:
            action = "run"
        rows.append(
            {
                "id": task.task_id,
                "repository_slot": task.repository_slot,
                "provider_slot": task.provider_slot,
                "action": action,
                "terminal_job_id": valid[0].job_id if valid else None,
                "invalid_terminal_artifacts": invalid,
                "ambiguous_artifacts": [str(path) for path in ambiguous_entries],
            }
        )
    return {
        "schema_version": 1,
        "mode": "plan",
        "provider_execution": False,
        "matrix_sha256": manifest.sha256,
        "artifacts": str(artifacts),
        "summary": {
            "total": len(rows),
            "skip_terminal": sum(row["action"] == "skip_terminal" for row in rows),
            "blocked_invalid": sum(
                row["action"] == "blocked_invalid" for row in rows
            ),
            "blocked_partial": sum(
                row["action"] == "blocked_partial" for row in rows
            ),
            "run": sum(row["action"] == "run" for row in rows),
        },
        "tasks": rows,
    }


def main() -> int:
    script_dir = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser(
        description=(
            "Plan the dogfood matrix without execution by default. --execute also "
            "requires DEADRECKON_DOGFOOD_EXECUTE=1."
        )
    )
    parser.add_argument(
        "--matrix",
        type=Path,
        default=Path(os.environ.get("DEADRECKON_DOGFOOD_MATRIX", script_dir / "matrix.json")),
    )
    parser.add_argument(
        "--artifacts",
        type=Path,
        default=Path(
            os.environ.get("DEADRECKON_DOGFOOD_ARTIFACTS", script_dir / "artifacts")
        ),
    )
    parser.add_argument("--runner", type=Path, default=script_dir / "run.sh")
    parser.add_argument(
        "--execute",
        action="store_true",
        help="run pending tasks after the matrix has been reviewed",
    )
    args = parser.parse_args()

    try:
        plan = build_plan(args.matrix, args.artifacts)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        parser.error(str(error))

    if not args.execute:
        print(json.dumps(plan, indent=2, sort_keys=True))
        return 0
    if os.environ.get("DEADRECKON_DOGFOOD_EXECUTE") != "1":
        parser.error(
            "--execute requires DEADRECKON_DOGFOOD_EXECUTE=1 after reviewing the plan"
        )

    plan["mode"] = "execute"
    plan["provider_execution"] = True
    print(json.dumps(plan, indent=2, sort_keys=True), flush=True)
    blocked = [
        task["id"]
        for task in plan["tasks"]
        if task["action"] in {"blocked_invalid", "blocked_partial"}
    ]
    if blocked:
        print(
            "refusing execution because prior task artifacts are incomplete, invalid, "
            "or ambiguous for: "
            + ", ".join(blocked)
            + "; inspect the recorded Job and archive or repair those artifacts before "
            "resuming",
            file=sys.stderr,
        )
        return 65
    runner_environment = os.environ.copy()
    runner_environment.update(
        {
            "DEADRECKON_DOGFOOD_MATRIX": str(args.matrix),
            "DEADRECKON_DOGFOOD_ARTIFACTS": str(args.artifacts),
            "DEADRECKON_DOGFOOD_MATRIX_SHA256": plan["matrix_sha256"],
        }
    )
    for task in plan["tasks"]:
        if task["action"] == "skip_terminal":
            continue
        completed = subprocess.run(
            [str(args.runner), task["id"]],
            check=False,
            env=runner_environment,
        )
        if completed.returncode != 0:
            print(
                f"dogfood task {task['id']} stopped with status {completed.returncode}; "
                "rerun the batch command to resume",
                file=sys.stderr,
            )
            return completed.returncode
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
