#!/usr/bin/env python3
"""Derive Watchkeeper dogfood metrics from persisted factual artifacts."""

from __future__ import annotations

import argparse
import collections
import datetime
import json
import math
import os
from pathlib import Path
from typing import Any

from dogfood_common import (
    load_matrix,
    read_json_object,
    validated_path_id,
    valid_terminal_observations,
    validated_task_artifact_root,
)


ADMINISTRATIVE_ARTIFACT_DIRS = {"live"}
TASK_ARTIFACT_NAMES = {
    "job-view.json",
    "operator-run.json",
    "start.json",
    "start.tmp.json",
    "status-latest.json",
}


def contains_task_artifacts(root: Path) -> bool:
    return any(
        path.is_file() and path.name in TASK_ARTIFACT_NAMES
        for path in root.rglob("*")
    )


def read_json(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as handle:
        value = json.load(handle)
    if not isinstance(value, dict):
        raise ValueError(f"{path} did not contain a JSON object")
    return value


def read_events(
    path: Path, missing: list[str], invalid_artifacts: list[str]
) -> tuple[list[dict[str, Any]], int]:
    events: list[dict[str, Any]] = []
    invalid = 0
    if path.is_symlink():
        invalid_artifacts.append(f"Job event history must not be a symlink: {path}")
        return events, invalid
    if not path.exists():
        missing.append(str(path))
        return events, invalid
    if not path.is_file():
        invalid_artifacts.append(f"Job event history is not a regular file: {path}")
        return events, invalid
    with path.open(encoding="utf-8") as handle:
        for raw in handle:
            if not raw.strip():
                continue
            try:
                value = json.loads(raw)
            except json.JSONDecodeError:
                invalid += 1
                continue
            if isinstance(value, dict):
                events.append(value)
            else:
                invalid += 1
    return events, invalid


def ratio(numerator: int, denominator: int) -> float | None:
    return round(numerator / denominator, 6) if denominator else None


def event_count(events: list[dict[str, Any]], kind: str) -> int:
    return sum(event.get("kind") == kind for event in events)


def elapsed_seconds(events: list[dict[str, Any]]) -> float | None:
    timestamps: list[datetime.datetime] = []
    for event in events:
        value = event.get("timestamp")
        if not isinstance(value, str):
            continue
        try:
            timestamps.append(datetime.datetime.fromisoformat(value.replace("Z", "+00:00")))
        except ValueError:
            continue
    if len(timestamps) < 2:
        return None
    return max(0.0, (max(timestamps) - min(timestamps)).total_seconds())


def stable_run_evidence_file(
    home: Path,
    path: Path,
    label: str,
    missing: list[str],
    invalid_artifacts: list[str],
) -> bool:
    runstate = home / "runstate"
    if runstate.is_symlink():
        invalid_artifacts.append(
            f"{label} must not traverse a symlink: {runstate}"
        )
        return False
    try:
        relative = path.relative_to(runstate)
    except ValueError:
        invalid_artifacts.append(f"{label} escapes DEADRECKON_HOME runstate: {path}")
        return False
    cursor = runstate
    for component in relative.parts:
        cursor = cursor / component
        if cursor.is_symlink():
            invalid_artifacts.append(f"{label} must not traverse a symlink: {cursor}")
            return False
    if not path.exists():
        missing.append(str(path))
        return False
    if not path.is_file():
        invalid_artifacts.append(f"{label} is not a regular file: {path}")
        return False
    return True


def semantic_spend(
    attempts: list[dict[str, Any]],
    home: Path,
    job_id: str,
    missing: list[str],
    invalid_artifacts: list[str],
) -> float:
    total = 0.0
    seen: set[Path] = set()
    for attempt in attempts:
        marker = attempt.get("proof", {}).get("marker_path")
        if not isinstance(marker, str):
            continue
        identity = attempt.get("id")
        if not isinstance(identity, dict):
            invalid_artifacts.append("semantic proof attempt omitted its Run identity")
            continue
        try:
            scope = validated_path_id(identity.get("scope"), "semantic proof scope")
            run_id = validated_path_id(identity.get("run_id"), "semantic proof run id")
        except ValueError as error:
            invalid_artifacts.append(str(error))
            continue
        expected_marker = (
            home
            / "runstate"
            / scope
            / "runs"
            / run_id
            / "proofs"
            / "turn-acceptance.json"
        )
        marker_path = Path(marker)
        if marker_path != expected_marker:
            invalid_artifacts.append(
                "semantic proof marker path does not match its JobView Run identity: "
                f"{marker_path}"
            )
            continue
        judgment_path = expected_marker.parent / "semantic-judgment.json"
        if judgment_path in seen:
            continue
        seen.add(judgment_path)
        if not stable_run_evidence_file(
            home, expected_marker, "deterministic marker", missing, invalid_artifacts
        ) or not stable_run_evidence_file(
            home,
            judgment_path,
            "semantic judgment",
            missing,
            invalid_artifacts,
        ):
            continue
        try:
            judgment = read_json(judgment_path)
        except (OSError, ValueError, json.JSONDecodeError) as error:
            invalid_artifacts.append(str(error))
            continue
        if judgment.get("job_id") != job_id or judgment.get("run_id") != run_id:
            invalid_artifacts.append(
                f"semantic judgment identity does not match Job {job_id} Run {run_id}: "
                f"{judgment_path}"
            )
            continue
        spend = judgment.get("spend_usd")
        if (
            isinstance(spend, bool)
            or not isinstance(spend, (int, float))
            or not math.isfinite(float(spend))
            or spend < 0
        ):
            invalid_artifacts.append(
                f"semantic judgment spend must be finite and nonnegative: {judgment_path}"
            )
            continue
        total += float(spend)
    return total


HUMAN_REVIEW_FIELDS = {
    "schema_version",
    "job_id",
    "reviewed_at",
    "reviewer",
    "false_acceptance",
    "false_rejection",
    "operator_interventions",
    "time_to_understand_seconds",
    "notes",
}


def load_review(observation_dir: Path, job_id: str) -> dict[str, Any] | None:
    path = observation_dir / "human-review.json"
    if path.is_symlink():
        raise ValueError(f"human review must not be a symlink: {path}")
    if not path.exists():
        return None
    if not path.is_file():
        raise ValueError(f"human review is not a regular file: {path}")
    review = read_json(path)
    if set(review) != HUMAN_REVIEW_FIELDS:
        missing = sorted(HUMAN_REVIEW_FIELDS - set(review))
        extra = sorted(set(review) - HUMAN_REVIEW_FIELDS)
        raise ValueError(
            f"{path} must contain exactly the human review fields; "
            f"missing={missing}, extra={extra}"
        )
    if type(review["schema_version"]) is not int or review["schema_version"] != 1:
        raise ValueError(f"{path} schema_version must be integer 1")
    if review["job_id"] != job_id:
        raise ValueError(f"{path} belongs to a different job")
    reviewed_at = review["reviewed_at"]
    if not isinstance(reviewed_at, str) or not reviewed_at.strip():
        raise ValueError(f"{path} reviewed_at must be a timezone-aware timestamp")
    try:
        timestamp = datetime.datetime.fromisoformat(reviewed_at.replace("Z", "+00:00"))
    except ValueError as error:
        raise ValueError(
            f"{path} reviewed_at must be a timezone-aware timestamp"
        ) from error
    if timestamp.tzinfo is None or timestamp.utcoffset() is None:
        raise ValueError(f"{path} reviewed_at must be a timezone-aware timestamp")
    reviewer = review["reviewer"]
    if not isinstance(reviewer, str) or not reviewer.strip():
        raise ValueError(f"{path} reviewer must be a non-empty string")
    for field in ("false_acceptance", "false_rejection"):
        if not isinstance(review[field], bool):
            raise ValueError(f"{path} {field} must be boolean")
    interventions = review["operator_interventions"]
    if type(interventions) is not int or interventions < 0:
        raise ValueError(
            f"{path} operator_interventions must be a nonnegative integer"
        )
    understand = review["time_to_understand_seconds"]
    if isinstance(understand, bool) or not isinstance(understand, (int, float)):
        raise ValueError(
            f"{path} time_to_understand_seconds must be finite and nonnegative"
        )
    if not math.isfinite(float(understand)) or understand < 0:
        raise ValueError(
            f"{path} time_to_understand_seconds must be finite and nonnegative"
        )
    if review["notes"] is not None and not isinstance(review["notes"], str):
        raise ValueError(f"{path} notes must be a string or null")
    return review


def load_job_report(
    observation_dir: Path, job_id: str, missing: list[str]
) -> dict[str, Any] | None:
    path = observation_dir / "job-report.json"
    if path.is_symlink():
        raise ValueError(f"Job report must not be a symlink: {path}")
    if not path.exists():
        missing.append(str(path))
        return None
    if not path.is_file():
        raise ValueError(f"Job report is not a regular file: {path}")
    report = read_json(path)
    if report.get("id") != job_id:
        raise ValueError(f"{path} belongs to a different job")
    return report


def report_proves_verified_completion(
    report: dict[str, Any] | None, job_id: str
) -> bool:
    if report is None:
        return False
    receipt = report.get("receipt")
    if not isinstance(receipt, dict):
        return False
    validated = receipt.get("receipt")
    return (
        report.get("id") == job_id
        and report.get("phase") == "terminal"
        and report.get("outcome") == "verified"
        and report.get("stop_reason") == "verified"
        and receipt.get("status") == "valid"
        and receipt.get("contained") is True
        and receipt.get("sandbox_backend") not in (None, "none")
        and receipt.get("signature_validation_error") is None
        and isinstance(validated, dict)
        and validated.get("job_id") == job_id
        and validated.get("proof_kind") == "two_key_completion"
    )


def operator_proves_successful_finish(
    record: dict[str, Any] | None, job_id: str
) -> bool:
    if record is None:
        return False
    public_commands = record.get("public_commands")
    return (
        record.get("job_id") == job_id
        and record.get("terminal_outcome") == "verified"
        and record.get("terminal_stop_reason") == "verified"
        and isinstance(public_commands, list)
        and "report" in public_commands
        and "finish" in public_commands
        and record.get("report_attempted") is True
        and record.get("report_exit_status") == 0
        and record.get("report_succeeded") is True
        and record.get("receipt_validation_attempted") is True
        and record.get("receipt_validation_source") == "deadreckon report --json"
        and record.get("receipt_validation_exit_status") == 0
        and record.get("receipt_validated") is True
        and record.get("finish_attempted") is True
        and record.get("finish_exit_status") == 0
        and record.get("finish_succeeded") is True
    )


def collect(home: Path, observations: Path, matrix_path: Path) -> dict[str, Any]:
    manifest = load_matrix(matrix_path)
    tasks_by_id = manifest.tasks_by_id
    attempted_task_ids: set[str] = set()
    completed_task_ids: set[str] = set()
    verified_task_ids: set[str] = set()
    reviewed_task_ids: set[str] = set()
    invalid_terminal_artifacts: list[str] = []
    invalid_human_reviews: list[str] = []
    invalid_event_artifacts: list[str] = []
    invalid_report_artifacts: list[str] = []
    invalid_semantic_artifacts: list[str] = []
    operator_record_paths: dict[str, Path] = {}

    if observations.is_dir():
        for entry in sorted(observations.iterdir()):
            task_id = entry.name
            if task_id in tasks_by_id:
                task_root = validated_task_artifact_root(observations, task_id)
                if any(task_root.iterdir()):
                    attempted_task_ids.add(task_id)
                continue
            if entry.is_dir() or entry.is_symlink():
                if task_id in ADMINISTRATIVE_ARTIFACT_DIRS:
                    continue
                if entry.is_symlink() or contains_task_artifacts(entry):
                    raise ValueError(
                        f"unknown observation task ID {task_id} at {entry}"
                    )
        for matrix_task_id in tasks_by_id:
            task_root = validated_task_artifact_root(observations, matrix_task_id)
            for operator_path in sorted(task_root.rglob("operator-run.json")):
                if operator_path.is_symlink():
                    raise ValueError(
                        f"operator observation must not be a symlink: {operator_path}"
                    )
                record = read_json_object(operator_path)
                task_id = record.get("task_id")
                if not isinstance(task_id, str) or task_id not in tasks_by_id:
                    raise ValueError(
                        f"{operator_path} contains unknown observation task ID "
                        f"{task_id!r}"
                    )
                if task_id != matrix_task_id:
                    raise ValueError(
                        f"{operator_path} task ID {task_id} disagrees with its "
                        "artifact directory"
                    )
                if task_id in operator_record_paths:
                    raise ValueError(
                        f"duplicate observation task ID {task_id}: "
                        f"{operator_record_paths[task_id]} and {operator_path}"
                    )
                operator_record_paths[task_id] = operator_path

    outcome_counts: collections.Counter[str] = collections.Counter()
    stop_counts: collections.Counter[str] = collections.Counter()
    confinement_counts: collections.Counter[str] = collections.Counter()
    jobs_observed = 0
    terminal_jobs = 0
    verified_jobs = 0
    unattended_verified_jobs = 0
    jobs_needing_recovery = 0
    automatically_recovered_jobs = 0
    retry_count = 0
    semantic_revision_count = 0
    operator_event_interventions = 0
    worker_spend_usd = 0.0
    worker_wall_seconds = 0.0
    judge_spend_usd = 0.0
    supervisor_elapsed_seconds = 0.0
    reviewed_jobs = 0
    false_acceptances = 0
    false_rejections = 0
    review_interventions = 0
    understanding_times: list[float] = []
    missing: list[str] = []
    invalid_event_rows = 0
    seen_jobs: set[str] = set()

    terminal_observations = []
    for task in manifest.tasks:
        task_root = validated_task_artifact_root(observations, task.task_id)
        valid, invalid = valid_terminal_observations(
            task_root, task, manifest.sha256
        )
        invalid_terminal_artifacts.extend(invalid)
        if len(valid) > 1:
            raise ValueError(
                f"duplicate observation task ID {task.task_id}: "
                + ", ".join(observation.job_id for observation in valid)
            )
        entries = sorted(task_root.iterdir()) if task_root.is_dir() else []
        valid_entry = valid[0].directory if valid else None
        for entry in entries:
            if entry != valid_entry:
                invalid_terminal_artifacts.append(
                    f"ambiguous extra artifact for matrix task {task.task_id}: {entry}"
                )
        terminal_observations.extend(valid)

    for observation in terminal_observations:
        status_path = observation.directory / "job-view.json"
        view = observation.view
        identity = view.get("job", {})
        projection = view.get("projection", {})
        job_id = observation.job_id
        if job_id in seen_jobs:
            raise ValueError(f"duplicate final JobView observation for {job_id}")
        seen_jobs.add(job_id)
        completed_task_ids.add(observation.task.task_id)
        jobs_observed += 1

        job_dir = home / "jobs" / job_id
        events, invalid = read_events(
            job_dir / "job-events.jsonl", missing, invalid_event_artifacts
        )
        invalid_event_rows += invalid
        operator_run = observation.operator_run
        report = None
        if operator_run.get("report_succeeded") is True:
            try:
                report = load_job_report(status_path.parent, job_id, missing)
            except (OSError, ValueError, json.JSONDecodeError) as error:
                invalid_report_artifacts.append(str(error))

        phase = projection.get("phase")
        outcome = projection.get("outcome")
        stop_reason = projection.get("stop_reason")
        if isinstance(outcome, str):
            outcome_counts[outcome] += 1
        if isinstance(stop_reason, str):
            stop_counts[stop_reason] += 1
        if phase == "terminal":
            terminal_jobs += 1

        verified = (
            phase == "terminal"
            and outcome == "verified"
            and stop_reason == "verified"
            and report_proves_verified_completion(report, job_id)
            and operator_proves_successful_finish(operator_run, job_id)
        )
        if verified:
            verified_jobs += 1
            verified_task_ids.add(observation.task.task_id)

        recoveries = event_count(events, "lease_reclaimed") + event_count(
            events, "retry_scheduled"
        )
        if recoveries:
            jobs_needing_recovery += 1
            if verified:
                automatically_recovered_jobs += 1
        retry_count += event_count(events, "retry_scheduled")
        semantic_revision_count += event_count(events, "semantic_judge_revise")
        event_interventions = event_count(events, "cancel_requested")
        operator_event_interventions += event_interventions

        attempts = view.get("attempts", [])
        if not isinstance(attempts, list):
            attempts = []
        for attempt in attempts:
            if not isinstance(attempt, dict):
                continue
            spend = attempt.get("spend", {})
            if isinstance(spend, dict):
                worker_spend_usd += float(spend.get("total_usd", 0.0))
                worker_wall_seconds += float(spend.get("wall_seconds", 0.0))
        judge_spend_usd += semantic_spend(
            attempts,
            home,
            job_id,
            missing,
            invalid_semantic_artifacts,
        )
        duration = elapsed_seconds(events)
        if duration is not None:
            supervisor_elapsed_seconds += duration

        if report is not None:
            receipt = report.get("receipt")
            if isinstance(receipt, dict) and receipt.get("status") == "valid":
                backend = str(receipt.get("sandbox_backend", "unknown"))
                confined = (
                    "contained" if receipt.get("contained") is True else "uncontained"
                )
                confinement_counts[f"{confined}:{backend}"] += 1
            elif isinstance(receipt, dict):
                confinement_counts[
                    f"unvalidated:{receipt.get('status', 'unknown')}"
                ] += 1

        try:
            review = load_review(status_path.parent, job_id)
        except (OSError, ValueError, json.JSONDecodeError) as error:
            invalid_human_reviews.append(str(error))
            review = None
        job_review_interventions = 0
        if review is not None:
            reviewed_jobs += 1
            reviewed_task_ids.add(observation.task.task_id)
            false_acceptances += review.get("false_acceptance") is True
            false_rejections += review.get("false_rejection") is True
            job_review_interventions = int(review.get("operator_interventions", 0))
            review_interventions += job_review_interventions
            understand = review.get("time_to_understand_seconds")
            if understand is not None:
                understanding_times.append(float(understand))

        if verified and event_interventions == 0 and job_review_interventions == 0:
            unattended_verified_jobs += 1

    total_interventions = operator_event_interventions + review_interventions
    human_measurements_available = reviewed_jobs > 0
    generated_at = datetime.datetime.now(datetime.timezone.utc).isoformat()
    all_task_ids = {task.task_id for task in manifest.tasks}
    missing_task_ids = all_task_ids - completed_task_ids
    completed_tasks = [tasks_by_id[task_id] for task_id in completed_task_ids]
    completed_repository_slots = sorted(
        {task.repository_slot for task in completed_tasks}
    )
    completed_provider_slots = sorted({task.provider_slot for task in completed_tasks})
    execution_complete = (
        20 <= len(manifest.tasks) <= 30
        and len(completed_task_ids) >= 20
        and not missing_task_ids
        and not invalid_terminal_artifacts
        and len(completed_repository_slots) >= 2
        and len(completed_provider_slots) >= 2
    )
    assessment_ready = (
        execution_complete
        and reviewed_task_ids == completed_task_ids
        and not invalid_human_reviews
        and not missing
        and not invalid_event_artifacts
        and not invalid_report_artifacts
        and not invalid_semantic_artifacts
        and invalid_event_rows == 0
    )
    return {
        "schema_version": 1,
        "generated_at": generated_at,
        "matrix": {
            "sha256": manifest.sha256,
            "task_count": len(manifest.tasks),
            "tasks": [
                {
                    "id": task.task_id,
                    "repository_slot": task.repository_slot,
                    "provider_slot": task.provider_slot,
                }
                for task in manifest.tasks
            ],
        },
        "campaign_completion": {
            "status": "complete" if execution_complete else "incomplete",
            "assessment_status": "ready" if assessment_ready else "incomplete",
            "claim_allowed": assessment_ready,
            "minimum_completed_tasks": 20,
            "completed_repository_slots": completed_repository_slots,
            "completed_provider_slots": completed_provider_slots,
            "completed_repository_slot_count": len(completed_repository_slots),
            "completed_provider_slot_count": len(completed_provider_slots),
            "counts": {
                "total": len(manifest.tasks),
                "missing": len(missing_task_ids),
                "attempted": len(attempted_task_ids),
                "completed": len(completed_task_ids),
                "verified": len(verified_task_ids),
                "reviewed": len(reviewed_task_ids),
            },
            "task_ids": {
                "missing": sorted(missing_task_ids),
                "attempted": sorted(attempted_task_ids),
                "completed": sorted(completed_task_ids),
                "verified": sorted(verified_task_ids),
                "reviewed": sorted(reviewed_task_ids),
            },
        },
        "basis": [
            "exact matrix bytes identified by SHA-256",
            "public status --json JobView observations",
            "public report --json output with authoritative receipt validation",
            "operator-run.json recording successful public finish",
            "$DEADRECKON_HOME/jobs/<id>/job-events.jsonl",
            "proofs/semantic-judgment.json when cited by an attempt",
            "structured human-review.json when supplied",
        ],
        "jobs_observed": jobs_observed,
        "persisted_facts": {
            "terminal_jobs": terminal_jobs,
            "verified_jobs": verified_jobs,
            "unattended_verified_jobs": unattended_verified_jobs,
            "unattended_verified_completion_rate": ratio(
                unattended_verified_jobs, jobs_observed
            ),
            "jobs_needing_recovery": jobs_needing_recovery,
            "automatically_recovered_jobs": automatically_recovered_jobs,
            "automatic_recovery_rate": ratio(
                automatically_recovered_jobs, jobs_needing_recovery
            ),
            "operator_event_interventions": operator_event_interventions,
            "worker_spend_usd": round(worker_spend_usd, 6),
            "worker_wall_seconds": round(worker_wall_seconds, 3),
            "judge_spend_usd": round(judge_spend_usd, 6),
            "supervisor_elapsed_seconds": round(supervisor_elapsed_seconds, 3),
            "retry_count": retry_count,
            "semantic_revision_count": semantic_revision_count,
            "outcome_distribution": dict(sorted(outcome_counts.items())),
            "stop_reason_distribution": dict(sorted(stop_counts.items())),
            "confinement_distribution": dict(sorted(confinement_counts.items())),
        },
        "human_review": {
            "reviewed_jobs": reviewed_jobs,
            "reviewed_task_ids": sorted(reviewed_task_ids),
            "false_acceptances": false_acceptances if human_measurements_available else None,
            "false_rejections": false_rejections if human_measurements_available else None,
            "operator_interventions": (
                total_interventions if human_measurements_available else None
            ),
            "mean_time_to_understand_seconds": (
                round(sum(understanding_times) / len(understanding_times), 3)
                if understanding_times
                else None
            ),
        },
        "data_quality": {
            "missing_factual_artifacts": sorted(set(missing)),
            "invalid_terminal_artifacts": sorted(set(invalid_terminal_artifacts)),
            "invalid_human_reviews": sorted(set(invalid_human_reviews)),
            "invalid_event_artifacts": sorted(set(invalid_event_artifacts)),
            "invalid_report_artifacts": sorted(set(invalid_report_artifacts)),
            "invalid_semantic_artifacts": sorted(set(invalid_semantic_artifacts)),
            "invalid_event_rows": invalid_event_rows,
            "narrative_files_consulted": 0,
        },
    }


def main() -> None:
    script_dir = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser()
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--observations", type=Path, required=True)
    parser.add_argument(
        "--matrix",
        type=Path,
        default=Path(
            os.environ.get("DEADRECKON_DOGFOOD_MATRIX", script_dir / "matrix.json")
        ),
    )
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    metrics = collect(args.home, args.observations, args.matrix)
    encoded = json.dumps(metrics, indent=2, sort_keys=True) + "\n"
    if args.output is None:
        print(encoded, end="")
    else:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded, encoding="utf-8")


if __name__ == "__main__":
    main()
