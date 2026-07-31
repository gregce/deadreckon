#!/usr/bin/env python3
"""Shared matrix and observation validation for Watchkeeper dogfood tools."""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
import json
import math
from pathlib import Path
import re
from typing import Any


SAFE_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}\Z")
ENV_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")


@dataclass(frozen=True)
class MatrixTask:
    task_id: str
    repository_slot: str
    provider_slot: str
    goal: str
    max_spend_usd: float


@dataclass(frozen=True)
class MatrixManifest:
    path: Path
    sha256: str
    tasks: tuple[MatrixTask, ...]
    repository_slots: frozenset[str]
    provider_slots: frozenset[str]
    repository_path_env: dict[str, str]
    provider_route_env: dict[str, str]

    @property
    def tasks_by_id(self) -> dict[str, MatrixTask]:
        return {task.task_id: task for task in self.tasks}


@dataclass(frozen=True)
class TerminalObservation:
    task: MatrixTask
    job_id: str
    directory: Path
    payload: dict[str, Any]
    view: dict[str, Any]
    operator_run: dict[str, Any]


def validated_task_artifact_root(artifact_root: Path, task_id: str) -> Path:
    task_root = artifact_root / task_id
    if task_root.is_symlink():
        raise ValueError(f"matrix task artifact root must not be a symlink: {task_root}")
    if task_root.exists() and not task_root.is_dir():
        raise ValueError(f"matrix task artifact root is not a directory: {task_root}")
    if task_root.is_dir():
        for candidate in task_root.iterdir():
            if candidate.is_symlink():
                raise ValueError(
                    f"matrix task artifact candidate must not be a symlink: {candidate}"
                )
    return task_root


def _required_string(value: object, label: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{label} must be a non-empty string")
    return value


def _safe_id(value: object, label: str) -> str:
    identity = _required_string(value, label)
    if SAFE_ID.fullmatch(identity) is None:
        raise ValueError(
            f"{label} must use only letters, digits, dot, underscore, or hyphen "
            "and must begin with a letter or digit"
        )
    return identity


def validated_path_id(value: object, label: str) -> str:
    """Return an identifier only when it is safe as one path component."""

    return _safe_id(value, label)


def _environment_name(value: object, label: str) -> str:
    name = _required_string(value, label)
    if ENV_NAME.fullmatch(name) is None:
        raise ValueError(f"{label} is not a safe environment variable name")
    return name


def _unique_slots(
    rows: object, label: str, environment_field: str
) -> tuple[frozenset[str], dict[str, str]]:
    if not isinstance(rows, list):
        raise ValueError(f"matrix {label} must be an array")
    slots: dict[str, str] = {}
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            raise ValueError(f"matrix {label}[{index}] must be an object")
        slot = _safe_id(row.get("slot"), f"matrix {label}[{index}].slot")
        if slot in slots:
            raise ValueError(f"matrix {label} contains duplicate slot {slot}")
        slots[slot] = _environment_name(
            row.get(environment_field),
            f"matrix {label}[{index}].{environment_field}",
        )
    return frozenset(slots), slots


def load_matrix(path: Path) -> MatrixManifest:
    raw = path.read_bytes()
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as error:
        raise ValueError(f"{path} is not valid JSON: {error}") from error
    if not isinstance(payload, dict):
        raise ValueError(f"{path} did not contain a JSON object")

    repository_slots, repository_path_env = _unique_slots(
        payload.get("repositories"), "repositories", "path_env"
    )
    provider_slots, provider_route_env = _unique_slots(
        payload.get("providers"), "providers", "route_env"
    )
    rows = payload.get("tasks")
    if not isinstance(rows, list) or not rows:
        raise ValueError("matrix tasks must be a non-empty array")
    if "task_count" in payload:
        task_count = payload["task_count"]
        if isinstance(task_count, bool) or not isinstance(task_count, int):
            raise ValueError("matrix task_count must be an integer when present")
        if task_count != len(rows):
            raise ValueError(
                f"matrix task_count is {task_count}, but tasks contains {len(rows)} rows"
            )

    tasks: list[MatrixTask] = []
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            raise ValueError(f"matrix tasks[{index}] must be an object")
        task_id = _safe_id(row.get("id"), f"matrix tasks[{index}].id")
        repository = _safe_id(
            row.get("repository"), f"matrix task {task_id}.repository"
        )
        provider = _safe_id(
            row.get("provider"), f"matrix task {task_id}.provider"
        )
        if repository not in repository_slots:
            raise ValueError(
                f"matrix task {task_id} references unknown repository slot {repository}"
            )
        if provider not in provider_slots:
            raise ValueError(
                f"matrix task {task_id} references unknown provider slot {provider}"
            )
        goal = _required_string(row.get("goal"), f"matrix task {task_id}.goal")
        raw_spend = row.get("max_spend_usd")
        if isinstance(raw_spend, bool):
            raise ValueError(
                f"matrix task {task_id}.max_spend_usd must be numeric"
            )
        try:
            max_spend_usd = float(raw_spend)
        except (TypeError, ValueError) as error:
            raise ValueError(
                f"matrix task {task_id}.max_spend_usd must be numeric"
            ) from error
        if not math.isfinite(max_spend_usd) or max_spend_usd < 0:
            raise ValueError(
                f"matrix task {task_id}.max_spend_usd must be finite and nonnegative"
            )
        tasks.append(
            MatrixTask(task_id, repository, provider, goal, max_spend_usd)
        )

    task_ids = [task.task_id for task in tasks]
    if len(task_ids) != len(set(task_ids)):
        raise ValueError("matrix contains duplicate task IDs")

    return MatrixManifest(
        path=path,
        sha256=f"sha256:{hashlib.sha256(raw).hexdigest()}",
        tasks=tuple(tasks),
        repository_slots=repository_slots,
        provider_slots=provider_slots,
        repository_path_env=repository_path_env,
        provider_route_env=provider_route_env,
    )


def read_json_object(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as handle:
        payload = json.load(handle)
    if not isinstance(payload, dict):
        raise ValueError(f"{path} did not contain a JSON object")
    return payload


def load_terminal_observation(
    directory: Path, task: MatrixTask, matrix_sha256: str
) -> TerminalObservation:
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError(f"Job observation candidate is not a real directory: {directory}")
    for name in ("job-view.json", "operator-run.json"):
        candidate = directory / name
        if candidate.is_symlink():
            raise ValueError(f"Job observation artifact must not be a symlink: {candidate}")
    payload = read_json_object(directory / "job-view.json")
    view = payload.get("job")
    if not isinstance(view, dict):
        raise ValueError(f"{directory}/job-view.json omitted the public JobView")
    identity = view.get("job")
    projection = view.get("projection")
    if not isinstance(identity, dict) or not isinstance(projection, dict):
        raise ValueError(f"{directory}/job-view.json omitted Job identity or projection")
    job_id = validated_path_id(identity.get("job_id"), "observed job_id")
    if directory.name != job_id:
        raise ValueError(
            f"{directory}/job-view.json belongs to Job {job_id}, not directory {directory.name}"
        )
    if projection.get("phase") != "terminal":
        raise ValueError(f"{directory}/job-view.json is not a terminal observation")

    operator_path = directory / "operator-run.json"
    operator_run = read_json_object(operator_path)
    expected = {
        "task_id": task.task_id,
        "job_id": job_id,
        "repository_slot": task.repository_slot,
        "provider_slot": task.provider_slot,
        "matrix_sha256": matrix_sha256,
    }
    for field, value in expected.items():
        if operator_run.get(field) != value:
            raise ValueError(
                f"{operator_path} field {field} was {operator_run.get(field)!r}, "
                f"expected {value!r}"
            )
    for field, projection_field in (
        ("terminal_outcome", "outcome"),
        ("terminal_stop_reason", "stop_reason"),
    ):
        if operator_run.get(field) != projection.get(projection_field):
            raise ValueError(
                f"{operator_path} field {field} disagrees with the terminal JobView"
            )
    return TerminalObservation(
        task=task,
        job_id=job_id,
        directory=directory,
        payload=payload,
        view=view,
        operator_run=operator_run,
    )


def observation_candidates(task_root: Path) -> list[Path]:
    if task_root.is_symlink():
        raise ValueError(f"matrix task artifact root must not be a symlink: {task_root}")
    if not task_root.is_dir():
        return []
    candidates: list[Path] = []
    for entry in task_root.iterdir():
        if entry.is_symlink():
            raise ValueError(f"matrix task artifact candidate must not be a symlink: {entry}")
        if not entry.is_dir():
            continue
        job_view = entry / "job-view.json"
        if job_view.is_symlink():
            raise ValueError(f"Job observation artifact must not be a symlink: {job_view}")
        if job_view.is_file():
            candidates.append(entry)
    return sorted(candidates)


def valid_terminal_observations(
    task_root: Path, task: MatrixTask, matrix_sha256: str
) -> tuple[list[TerminalObservation], list[str]]:
    valid: list[TerminalObservation] = []
    invalid: list[str] = []
    for directory in observation_candidates(task_root):
        try:
            valid.append(load_terminal_observation(directory, task, matrix_sha256))
        except (OSError, ValueError, json.JSONDecodeError) as error:
            invalid.append(str(error))
    return valid, invalid
