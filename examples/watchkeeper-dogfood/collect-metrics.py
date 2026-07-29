#!/usr/bin/env python3
"""Derive Watchkeeper dogfood metrics from persisted factual artifacts."""

from __future__ import annotations

import argparse
import collections
import datetime
import json
from pathlib import Path
from typing import Any


def read_json(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as handle:
        value = json.load(handle)
    if not isinstance(value, dict):
        raise ValueError(f"{path} did not contain a JSON object")
    return value


def read_events(path: Path) -> tuple[list[dict[str, Any]], int]:
    events: list[dict[str, Any]] = []
    invalid = 0
    if not path.is_file():
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


def semantic_spend(attempts: list[dict[str, Any]], missing: list[str]) -> float:
    total = 0.0
    seen: set[Path] = set()
    for attempt in attempts:
        marker = attempt.get("proof", {}).get("marker_path")
        if not isinstance(marker, str):
            continue
        judgment_path = Path(marker).parent / "semantic-judgment.json"
        if judgment_path in seen:
            continue
        seen.add(judgment_path)
        if not judgment_path.is_file():
            missing.append(str(judgment_path))
            continue
        judgment = read_json(judgment_path)
        total += float(judgment.get("spend_usd", 0.0))
    return total


def load_review(observation_dir: Path, job_id: str) -> dict[str, Any] | None:
    path = observation_dir / "human-review.json"
    if not path.is_file():
        return None
    review = read_json(path)
    if review.get("job_id") != job_id:
        raise ValueError(f"{path} belongs to a different job")
    return review


def load_job_report(
    observation_dir: Path, job_id: str, missing: list[str]
) -> dict[str, Any] | None:
    path = observation_dir / "job-report.json"
    if not path.is_file():
        missing.append(str(path))
        return None
    report = read_json(path)
    if report.get("id") != job_id:
        raise ValueError(f"{path} belongs to a different job")
    return report


def load_operator_run(
    observation_dir: Path, job_id: str, missing: list[str]
) -> dict[str, Any] | None:
    path = observation_dir / "operator-run.json"
    if not path.is_file():
        missing.append(str(path))
        return None
    record = read_json(path)
    if record.get("job_id") != job_id:
        raise ValueError(f"{path} belongs to a different job")
    return record


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


def collect(home: Path, observations: Path) -> dict[str, Any]:
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

    for status_path in sorted(observations.rglob("job-view.json")):
        payload = read_json(status_path)
        view = payload.get("job")
        if not isinstance(view, dict):
            raise ValueError(f"{status_path} omitted the public status JobView")
        identity = view.get("job", {})
        projection = view.get("projection", {})
        job_id = identity.get("job_id")
        if not isinstance(job_id, str) or not job_id:
            raise ValueError(f"{status_path} omitted job.job_id")
        if job_id in seen_jobs:
            raise ValueError(f"duplicate final JobView observation for {job_id}")
        seen_jobs.add(job_id)
        jobs_observed += 1

        job_dir = home / "jobs" / job_id
        events, invalid = read_events(job_dir / "job-events.jsonl")
        invalid_event_rows += invalid
        report = load_job_report(status_path.parent, job_id, missing)
        operator_run = load_operator_run(status_path.parent, job_id, missing)

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
        judge_spend_usd += semantic_spend(attempts, missing)
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

        review = load_review(status_path.parent, job_id)
        job_review_interventions = 0
        if review is not None:
            reviewed_jobs += 1
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
    return {
        "schema_version": 1,
        "generated_at": generated_at,
        "basis": [
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
            "invalid_event_rows": invalid_event_rows,
            "narrative_files_consulted": 0,
        },
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--observations", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    metrics = collect(args.home, args.observations)
    encoded = json.dumps(metrics, indent=2, sort_keys=True) + "\n"
    if args.output is None:
        print(encoded, end="")
    else:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded, encoding="utf-8")


if __name__ == "__main__":
    main()
