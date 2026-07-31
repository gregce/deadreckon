#!/usr/bin/env python3
"""Record and evaluate operator-run Watchkeeper live fault trials.

This program never starts a provider, changes host networking, installs or
controls a service, reboots a machine, signals a process, or calls ``finish``.
The operator performs reviewed interventions separately. In trusted mode this
recorder asks only ``dr-capture`` for canonical observations and a protected
receipt; the legacy manual-copy mode remains documentation-only.
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import math
import os
import platform
import re
import secrets
import shutil
import stat
import subprocess
import sys
import tempfile
import uuid
from pathlib import Path
from typing import Any


SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_MANIFEST = SCRIPT_DIR / "live-trials.json"
RESULT_SCHEMA = SCRIPT_DIR / "live-trial-results.schema.json"
STATE_FILE = "trial-state.json"
REPLAY_FILE = "replay.json"
RAW_DIR = "raw"
EXPECTED_CLAIM_IDS = {
    "live_provider_worker_kill",
    "live_provider_supervisor_restart",
    "live_provider_network_loss",
    "machine_reboot",
    "cross_provider_gate_attack",
    "live_provider_parent_repair",
    "live_campaign_interruption_recovery",
    "linux_bubblewrap_gate_boundary",
    "docker_gate_boundary",
}
FORMAT_SUFFIX = {"json": ".json", "jsonl": ".jsonl", "text": ".txt"}
MEDIA_TYPE = {
    "json": "application/json",
    "jsonl": "application/x-ndjson",
    "text": "text/plain; charset=utf-8",
}
ALLOWED_EVIDENCE_SOURCES = {
    "job-view",
    "job-events",
    "job-intervention",
    "job-cleanup",
    "job",
    "authority",
    "launch-plan",
    "lease",
    "job-report",
    "receipt",
    "supervised-child",
    "host-boot-id",
    "semantic-judgment",
    "parent-repair-manifest",
    "parent-repair-candidate",
    "doctor",
    "supervisor-service-status",
    "parent-artifact",
    "parent-events",
    "campaign",
    "campaign-events",
    "active-plan",
    "active-plan-events",
    "unavailable-objective",
}
TRUSTED_INTERVENTION_SOURCES = {
    "job-intervention",
    "campaign-intervention",
    "sandbox-boundary-observation",
}
RECORDER_COMMAND = "python3 examples/watchkeeper-dogfood/live-trial.py"
ALLOWED_BACKENDS = {"sandbox-exec", "bwrap", "docker", "none", "unknown"}
SUPERVISED_IDENTITY_FIELDS = (
    "pid",
    "launch_id",
    "attempt",
    "release_token_sha256",
    "boot_id",
    "process_start_identity",
)
MAX_IMMUTABLE_ARTIFACT_BYTES = 256 * 1024 * 1024
SANDBOX_BOUNDARY_OBSERVATION_REQUIRED_FIELDS = {
    "schema_version",
    "job_id",
    "run_id",
    "observed_at",
    "issuer",
    "probe_id",
    "attempt",
    "outer_launch_id",
    "authority_sha256",
    "contract_sha256",
    "result_tree_sha256",
    "sandbox_requested",
    "sandbox_backend",
    "contained",
    "gate_key_read_denied",
    "proof_write_denied",
    "control_write_denied",
    "operator_capture_read_denied",
    "operator_capture_write_denied",
    "signing_env_scrubbed",
    "probe_sha256",
    "signature",
}
SANDBOX_BOUNDARY_OBSERVATION_OPTIONAL_FIELDS = {"gate_evaluator_sha256"}
JOB_AUTHORITY_REQUIRED_FIELDS = {
    "schema_version",
    "job_id",
    "run_id",
    "approved_at",
    "accepted_by",
    "goal_sha256",
    "contract_sha256",
    "effective_policy_sha256",
    "launch_plan_sha256",
    "source_tree_sha256",
    "source_revision",
    "sandbox_requested",
    "semantic_judge_mode",
}
JOB_AUTHORITY_OPTIONAL_FIELDS = {"gate_evaluator_sha256"}
SANDBOX_BOUNDARY_DENIAL_FIELDS = (
    "contained",
    "gate_key_read_denied",
    "proof_write_denied",
    "control_write_denied",
    "operator_capture_read_denied",
    "operator_capture_write_denied",
    "signing_env_scrubbed",
)


class TrialError(RuntimeError):
    """A safe, operator-actionable recorder error."""


def now() -> str:
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def canonical_bytes(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")


def pretty_json_bytes(value: Any) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode("utf-8")


def digest_bytes(data: bytes) -> str:
    return f"sha256:{hashlib.sha256(data).hexdigest()}"


def digest_value(value: Any) -> str:
    return digest_bytes(canonical_bytes(value))


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(
            path.read_text(encoding="utf-8"),
            parse_constant=lambda token: (_ for _ in ()).throw(
                ValueError(f"non-finite JSON number {token}")
            ),
        )
    except (OSError, ValueError) as error:
        raise TrialError(f"{path.name} is not valid readable JSON: {error}") from error
    if not isinstance(value, dict):
        raise TrialError(f"{path.name} must contain one JSON object")
    return value


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(pretty_json_bytes(value))


def write_json_no_clobber(path: Path, value: Any) -> None:
    publish_bytes_no_clobber(path, pretty_json_bytes(value))


def stable_regular_bytes(path: Path, label: str) -> bytes:
    try:
        before = os.stat(path, follow_symlinks=False)
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(path, flags)
        try:
            opened = os.fstat(descriptor)
            if (
                not stat.S_ISREG(before.st_mode)
                or not stat.S_ISREG(opened.st_mode)
                or opened.st_size > MAX_IMMUTABLE_ARTIFACT_BYTES
                or (before.st_dev, before.st_ino) != (opened.st_dev, opened.st_ino)
            ):
                raise TrialError(
                    f"{label} must be a stable regular non-symlink file"
                )
            with os.fdopen(descriptor, "rb", closefd=False) as handle:
                data = handle.read(MAX_IMMUTABLE_ARTIFACT_BYTES + 1)
            after = os.fstat(descriptor)
        finally:
            os.close(descriptor)
        post = os.stat(path, follow_symlinks=False)
    except (FileNotFoundError, OSError) as error:
        raise TrialError(f"{label} is not a stable readable file: {error}") from error
    identity = lambda item: (
        item.st_dev,
        item.st_ino,
        item.st_size,
        item.st_mtime_ns,
    )
    if (
        len(data) > MAX_IMMUTABLE_ARTIFACT_BYTES
        or identity(before) != identity(opened)
        or identity(opened) != identity(after)
        or identity(after) != identity(post)
        or len(data) != after.st_size
    ):
        raise TrialError(f"{label} changed while it was being read")
    return data


def sync_directory(path: Path) -> None:
    if os.name == "nt":
        return
    flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0)
    descriptor = os.open(path, flags)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def publish_bytes_no_clobber(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor = -1
    temp_name: str | None = None
    try:
        descriptor, temp_name = tempfile.mkstemp(
            prefix=f".{path.name}.deadreckon-capture-",
            suffix=".tmp",
            dir=path.parent,
        )
        with os.fdopen(descriptor, "wb") as handle:
            descriptor = -1
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        try:
            os.link(temp_name, path, follow_symlinks=False)
        except FileExistsError as error:
            try:
                existing = stable_regular_bytes(path, path.name)
            except TrialError as unsafe:
                raise TrialError(
                    f"{path.name} already exists but is unsafe: {unsafe}"
                ) from error
            if existing != data:
                raise TrialError(
                    f"{path.name} already exists with different bytes"
                ) from error
            return
        published = os.open(
            path,
            os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0),
        )
        try:
            os.fsync(published)
        finally:
            os.close(published)
        sync_directory(path.parent)
    except TrialError:
        raise
    except OSError as error:
        raise TrialError(f"cannot publish immutable {path.name}: {error}") from error
    finally:
        if descriptor >= 0:
            os.close(descriptor)
        if temp_name is not None:
            try:
                os.unlink(temp_name)
            except FileNotFoundError:
                pass


def stable_regular_path(path: Path, label: str) -> Path:
    if not path.is_file() or path.is_symlink():
        raise TrialError(f"{label} must be an existing regular non-symlink file")
    return path.resolve()


def run_capture_helper(state: dict[str, Any], command: list[str]) -> dict[str, Any]:
    capture = state.get("trusted_capture")
    if not isinstance(capture, dict):
        raise TrialError("trusted capture state is absent")
    helper = stable_regular_path(Path(str(capture.get("helper"))), "capture helper")
    completed = subprocess.run(
        [str(helper), *command],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        action = command[0] if command else "command"
        raise TrialError(
            f"trusted capture helper {action} failed closed "
            f"(exit {completed.returncode})"
        )
    try:
        value = json.loads(completed.stdout)
    except ValueError as error:
        raise TrialError("trusted capture helper returned malformed JSON") from error
    if not isinstance(value, dict):
        raise TrialError("trusted capture helper must return one JSON object")
    return value


def helper_identity(state: dict[str, Any]) -> list[str]:
    capture = state.get("trusted_capture")
    if not isinstance(capture, dict):
        raise TrialError("trusted capture state is absent")
    job_id = capture.get("job_id")
    session_id = state.get("session_id")
    if not isinstance(job_id, str) or not job_id or not isinstance(session_id, str):
        raise TrialError("trusted capture identity is malformed")
    return ["--job-id", job_id, "--session-id", session_id]


def normalized_helper_name(value: Any) -> str:
    return str(value).replace("_", "-").lower()


def load_manifest(path: Path) -> tuple[dict[str, Any], dict[str, dict[str, Any]]]:
    manifest = load_json(path)
    trials = manifest.get("trials")
    claim_ids = manifest.get("claim_ids")
    if not isinstance(trials, list) or not isinstance(claim_ids, list):
        raise TrialError("live-trials.json must contain claim_ids and trials arrays")
    indexed: dict[str, dict[str, Any]] = {}
    for trial in trials:
        if not isinstance(trial, dict) or not isinstance(trial.get("id"), str):
            raise TrialError("every live trial must be an object with an id")
        trial_id = trial["id"]
        if trial_id in indexed:
            raise TrialError(f"duplicate live trial id: {trial_id}")
        indexed[trial_id] = trial
    if set(claim_ids) != EXPECTED_CLAIM_IDS or set(indexed) != EXPECTED_CLAIM_IDS:
        raise TrialError("manifest must cover exactly the nine current live claim ids")
    if len(claim_ids) != len(EXPECTED_CLAIM_IDS) or len(indexed) != len(
        EXPECTED_CLAIM_IDS
    ):
        raise TrialError("manifest claim ids must be unique")
    trusted_capture = manifest.get("trusted_capture")
    if not isinstance(trusted_capture, dict):
        raise TrialError("manifest must declare trusted_capture guidance")
    intervention_sources = trusted_capture.get("intervention_sources")
    if (
        trusted_capture.get("helper") != "dr-capture"
        or trusted_capture.get("cleanup_source") != "job-cleanup"
        or not isinstance(intervention_sources, dict)
        or set(intervention_sources) != EXPECTED_CLAIM_IDS
        or not set(intervention_sources.values()).issubset(
            TRUSTED_INTERVENTION_SOURCES
        )
        or not valid_digest(trusted_capture.get("sandbox_boundary_probe_sha256"))
    ):
        raise TrialError("manifest trusted_capture guidance is malformed")
    return manifest, indexed


def state_path(trial_dir: Path) -> Path:
    return trial_dir / STATE_FILE


def load_state(trial_dir: Path) -> dict[str, Any]:
    path = state_path(trial_dir)
    if not path.is_file() or path.is_symlink():
        raise TrialError(f"{STATE_FILE} is absent; run prepare first")
    return load_json(path)


def save_state(trial_dir: Path, state: dict[str, Any]) -> None:
    write_json(state_path(trial_dir), state)


def validate_revision(revision: str) -> str:
    normalized = revision.strip().lower()
    if re.fullmatch(r"[0-9a-f]{7,40}", normalized) is None:
        raise TrialError("source revision must be a 7-40 character Git hex id")
    return normalized


def evidence_index(trial: dict[str, Any]) -> dict[str, dict[str, Any]]:
    evidence = trial.get("evidence")
    if not isinstance(evidence, list):
        raise TrialError(f"trial {trial['id']} has no evidence declarations")
    indexed: dict[str, dict[str, Any]] = {}
    for declaration in evidence:
        if not isinstance(declaration, dict):
            raise TrialError(f"trial {trial['id']} has a malformed evidence declaration")
        name = declaration.get("name")
        source = declaration.get("source")
        evidence_format = declaration.get("format")
        if (
            not isinstance(name, str)
            or re.fullmatch(r"[a-z][a-z0-9-]*", name) is None
            or source not in ALLOWED_EVIDENCE_SOURCES
            or evidence_format not in FORMAT_SUFFIX
        ):
            raise TrialError(f"trial {trial['id']} has invalid evidence metadata")
        if name in indexed:
            raise TrialError(f"trial {trial['id']} repeats evidence {name}")
        indexed[name] = declaration
    phases = trial.get("capture_phases")
    if not isinstance(phases, dict) or set(phases) != {"before", "after"}:
        raise TrialError(
            f"trial {trial['id']} must declare exact before/after capture phases"
        )
    phase_names: list[str] = []
    for phase in ("before", "after"):
        names = phases[phase]
        if not isinstance(names, list) or not all(
            isinstance(name, str) for name in names
        ):
            raise TrialError(f"trial {trial['id']} has malformed {phase} captures")
        phase_names.extend(names)
    if len(phase_names) != len(set(phase_names)) or set(phase_names) != set(indexed):
        raise TrialError(
            f"trial {trial['id']} capture phases must partition declared evidence"
        )
    return indexed


def command_prepare(
    manifest_path: Path,
    trial_id: str,
    trial_dir: Path,
    revision: str,
    *,
    capture_helper: Path | None = None,
    deadreckon_binary: Path | None = None,
    backend: str | None = None,
    provider_routes: list[str] | None = None,
    job_id: str | None = None,
) -> None:
    manifest, trials = load_manifest(manifest_path)
    trial = trials.get(trial_id)
    if trial is None:
        raise TrialError(f"unknown live trial: {trial_id}")
    revision = validate_revision(revision)
    provider_routes = provider_routes or []
    trusted_values = (
        capture_helper,
        deadreckon_binary,
        backend,
        job_id,
    )
    trusted = any(value is not None for value in trusted_values) or bool(provider_routes)
    if trusted and (
        any(value is None for value in trusted_values)
        or not provider_routes
        or backend not in {"sandbox-exec", "bwrap", "docker"}
        or not isinstance(job_id, str)
        or not job_id.strip()
    ):
        raise TrialError(
            "trusted prepare requires --capture-helper, --deadreckon-binary, "
            "--job-id, a contained --backend, and at least one --provider-route"
        )
    if len(set(provider_routes)) != len(provider_routes) or any(
        not route.strip() for route in provider_routes
    ):
        raise TrialError("trusted provider routes must be unique and non-empty")
    if trusted:
        declared_roles: dict[str, list[str]] = {}
        for declaration in provider_routes:
            role, separator, route = declaration.partition("=")
            if not separator or not role or not route:
                raise TrialError("--provider-route must use ROLE=ROUTE")
            declared_roles.setdefault(role, []).append(route)
        expected_roles = set(trial["job"]["provider_slots"])
        if set(declared_roles) != expected_roles:
            raise TrialError(
                "trusted provider routes must cover exactly the trial provider slots"
            )
        judge_routes = set(declared_roles["independent_judge"])
        worker_routes = set(
            declared_roles.get("worker", [])
            + declared_roles.get("hostile_worker", [])
        )
        if not judge_routes.isdisjoint(worker_routes):
            raise TrialError(
                "independent judge routes must be distinct from worker routes"
            )
    trusted_config = None
    if trusted:
        assert capture_helper is not None
        assert deadreckon_binary is not None
        assert backend is not None
        assert job_id is not None
        trusted_config = {
            "helper": str(stable_regular_path(capture_helper, "capture helper")),
            "deadreckon_binary": str(
                stable_regular_path(deadreckon_binary, "DeadReckon binary")
            ),
            "recorder_interpreter": str(
                stable_regular_path(
                    Path(sys.executable).resolve(),
                    "recorder interpreter",
                )
            ),
            "job_id": job_id.strip(),
            "backend": backend,
            "provider_routes": provider_routes,
        }
    trial_dir.mkdir(parents=True, exist_ok=True)
    existing = state_path(trial_dir).exists() or state_path(trial_dir).is_symlink()
    if existing and not trusted:
        raise TrialError(f"{STATE_FILE} already exists; use a new trial directory")
    raw_dir = trial_dir / RAW_DIR
    if raw_dir.is_symlink() or (raw_dir.exists() and not raw_dir.is_dir()):
        raise TrialError("raw capture path must be a non-symlink directory")
    raw_dir.mkdir(exist_ok=True)
    declarations = evidence_index(trial)
    if existing:
        state = load_state(trial_dir)
        provenance_status = state.get("capture_provenance", {}).get("status")
        if (
            state.get("schema_version") != 2
            or state.get("trial_id") != trial_id
            or state.get("source_revision") != revision
            or state.get("capture_mode") != "trusted"
            or state.get("trusted_capture") != trusted_config
            or provenance_status not in {"trusted_pending", "trusted_prepared"}
            or not isinstance(state.get("session_id"), str)
            or not state["session_id"]
        ):
            raise TrialError(
                "existing trusted prepare state conflicts with requested inputs"
            )
    else:
        state = {
            "schema_version": 2,
            "trial_id": trial_id,
            "session_id": secrets.token_hex(16),
            "source_revision": revision,
            "created_at": now(),
            "capture_mode": "trusted" if trusted else "operator_attested",
            "capture_provenance": {
                "status": "trusted_pending" if trusted else "operator_attested",
                "receipt_sha256": None,
                "reason": (
                    "trusted canonical capture is not verified until seal and verify"
                    if trusted
                    else (
                        "manual captures are documentation only; a trusted DeadReckon "
                        "capture receipt is required for pass"
                    )
                ),
            },
            "captures": {},
            "intervention": {
                "kind": trial["intervention"]["kind"],
                "status": "not_performed",
                "recorded_at": None,
                "detail_sha256": None,
            },
            "cleanup": {
                "status": "not_run",
                "recorded_at": None,
                "detail_sha256": None,
            },
        }
        if trusted:
            state["trusted_capture"] = trusted_config
        save_state(trial_dir, state)
    before_names = [
        name
        for name, declaration in declarations.items()
        if declaration.get("required") is True
        and evidence_phase(trial, name) == "before"
    ]
    after_names = [
        name
        for name, declaration in declarations.items()
        if declaration.get("required") is True
        and evidence_phase(trial, name) == "after"
    ]

    def capture_command(names: list[str]) -> str:
        if trusted:
            captures = " ".join(f"--canonical {name}" for name in names)
        else:
            captures = " ".join(
                f"--capture {name}=/path/to/{name}{FORMAT_SUFFIX[declarations[name]['format']]}"
                for name in names
            )
        return (
            f"{RECORDER_COMMAND} observe --trial-dir \"$WK_LIVE_TRIAL\" {captures}"
        ).rstrip()

    replay = {
        "schema_version": 2,
        "trial_id": trial_id,
        "source_revision": revision,
        "capture_mode": state["capture_mode"],
        "operator_intervention_only": True,
        "required_evidence": [
            declaration["name"]
            for declaration in declarations.values()
            if declaration.get("required") is True
        ],
        "trusted_sources": {
            "intervention": manifest["trusted_capture"]["intervention_sources"][
                trial_id
            ],
            "cleanup": manifest["trusted_capture"]["cleanup_source"],
        },
        "intervention_instructions": trial["intervention"]["instructions"],
        "observe_command": capture_command(before_names),
        "before_observe_command": capture_command(before_names),
        "intervention_record_command": (
            f"{RECORDER_COMMAND} observe --trial-dir \"$WK_LIVE_TRIAL\" "
            "--intervention-status performed --intervention-detail-file "
            "/path/to/operator-intervention-evidence"
        ),
        "after_observe_command": capture_command(after_names),
        "finalize_command": (
            f"{RECORDER_COMMAND} finalize --trial-dir \"$WK_LIVE_TRIAL\" "
            "--output \"$WK_LIVE_TRIAL/result.json\" "
            "--evaluation-output \"$WK_LIVE_TRIAL/result.evaluation.json\""
        ),
        "cleanup_instructions": trial["cleanup"],
        "cleanup_record_command": (
            f"{RECORDER_COMMAND} cleanup --trial-dir \"$WK_LIVE_TRIAL\" "
            "--status completed --detail-file /path/to/cleanup-evidence"
        ),
    }
    replay_path = trial_dir / REPLAY_FILE
    if replay_path.exists() or replay_path.is_symlink():
        existing_replay_bytes = stable_regular_bytes(
            replay_path, "existing immutable replay"
        )
        try:
            existing_replay = json.loads(existing_replay_bytes)
        except ValueError as error:
            raise TrialError("existing immutable replay is malformed") from error
        if (
            existing_replay != replay
            or existing_replay_bytes != pretty_json_bytes(replay)
        ):
            raise TrialError("existing replay conflicts with trusted prepare inputs")
    else:
        write_json_no_clobber(replay_path, replay)
    if trusted:
        assert capture_helper is not None
        assert deadreckon_binary is not None
        assert backend is not None
        response = run_capture_helper(
            state,
            [
                "prepare",
                *helper_identity(state),
                "--trial-id",
                trial_id,
                "--manifest",
                str(stable_regular_path(manifest_path, "manifest")),
                "--result-schema",
                str(stable_regular_path(RESULT_SCHEMA, "result schema")),
                "--recorder",
                str(stable_regular_path(Path(__file__), "recorder")),
                "--recorder-interpreter",
                state["trusted_capture"]["recorder_interpreter"],
                "--deadreckon-binary",
                str(stable_regular_path(deadreckon_binary, "DeadReckon binary")),
                "--replay",
                str(
                    stable_regular_path(
                        trial_dir / REPLAY_FILE,
                        "replay",
                    )
                ),
                "--backend",
                backend,
                *[
                    item
                    for route in provider_routes
                    for item in ("--provider-route", route)
                ],
            ],
        )
        if (
            response.get("job_id") != job_id
            or response.get("session_id") != state["session_id"]
            or response.get("trial_id") != trial_id
            or response.get("source_revision") != revision
        ):
            raise TrialError("trusted capture prepare returned a mismatched binding")
        state["capture_provenance"]["status"] = "trusted_prepared"
        save_state(trial_dir, state)
    print(trial_dir)


def validate_capture(path: Path, evidence_format: str) -> None:
    if evidence_format == "json":
        load_json(path)
        return
    if evidence_format == "jsonl":
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except OSError as error:
            raise TrialError(f"cannot read {path.name}: {error}") from error
        for line_number, raw in enumerate(lines, start=1):
            if not raw.strip():
                continue
            try:
                value = json.loads(
                    raw,
                    parse_constant=lambda token: (_ for _ in ()).throw(
                        ValueError(f"non-finite JSON number {token}")
                    ),
                )
            except ValueError as error:
                raise TrialError(
                    f"{path.name}:{line_number} is not valid JSON: {error}"
                ) from error
            if not isinstance(value, dict):
                raise TrialError(f"{path.name}:{line_number} must be a JSON object")
        return
    try:
        path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        raise TrialError(f"{path.name} is not readable UTF-8 text: {error}") from error


def parse_capture(value: str) -> tuple[str, Path]:
    name, separator, raw_path = value.partition("=")
    if not separator or not name or not raw_path:
        raise TrialError("--capture must use NAME=/path/to/file")
    return name, Path(raw_path)


def evidence_phase(trial: dict[str, Any], name: str) -> str:
    phases = trial["capture_phases"]
    for phase in ("before", "after"):
        if name in phases[phase]:
            return phase
    raise TrialError(f"{name} has no explicit capture phase for {trial['id']}")


def optional_detail_digest(path: Path | None) -> str | None:
    if path is None:
        return None
    if not path.is_file() or path.is_symlink():
        raise TrialError("detail file must be an existing regular non-symlink file")
    return digest_bytes(path.read_bytes())


def trusted_observation(
    state: dict[str, Any],
    trial_dir: Path,
    *,
    source: str,
    subject: str,
    phase: str,
    target_name: str,
    evidence_format: str,
) -> dict[str, Any]:
    target = trial_dir / RAW_DIR / target_name
    if target.is_symlink() or (target.exists() and not target.is_file()):
        raise TrialError(f"canonical capture target for {subject} is unsafe")
    response = run_capture_helper(
        state,
        [
            "observe",
            *helper_identity(state),
            "--source",
            source,
            "--subject",
            subject,
            "--event-id",
            f"watchkeeper:{state['session_id']}:{phase}:{subject}",
            "--causation-id",
            f"watchkeeper:{state['trial_id']}:{phase}",
            "--phase",
            phase,
            "--output",
            str(target),
        ],
    )
    if (
        response.get("job_id") != state["trusted_capture"]["job_id"]
        or response.get("session_id") != state["session_id"]
        or response.get("subject") != subject
        or normalized_helper_name(response.get("source")) != source
        or normalized_helper_name(response.get("phase")) != phase
    ):
        raise TrialError("trusted capture helper returned a mismatched observation")
    if not target.is_file() or target.is_symlink():
        raise TrialError("trusted capture helper did not create canonical evidence")
    validate_capture(target, evidence_format)
    data = target.read_bytes()
    sha256 = digest_bytes(data)
    if response.get("content_sha256") != sha256 or response.get(
        "content_bytes"
    ) != len(data):
        raise TrialError("canonical output does not match its trusted capture event")
    timestamp = response.get("timestamp")
    if not isinstance(timestamp, str):
        raise TrialError("trusted capture event timestamp is malformed")
    return {
        "file": target_name,
        "format": evidence_format,
        "captured_at": timestamp,
        "bytes": len(data),
        "sha256": sha256,
        "provenance": "trusted_canonical",
        "source": source,
        "phase": phase,
    }


def command_observe(
    manifest_path: Path,
    trial_dir: Path,
    captures: list[str],
    canonical_subjects: list[str],
    replace: bool,
    intervention_status: str | None,
    intervention_detail_file: Path | None,
) -> None:
    if replace:
        raise TrialError(
            "--replace is forbidden; live evidence is append-only, so use a new trial directory"
        )
    manifest, trials = load_manifest(manifest_path)
    state = load_state(trial_dir)
    trial = trials[state["trial_id"]]
    trusted = state.get("capture_mode") == "trusted"
    if trusted and captures:
        raise TrialError("trusted mode accepts --canonical, not manual --capture")
    if not trusted and canonical_subjects:
        raise TrialError("manual mode accepts --capture, not --canonical")
    if len(canonical_subjects) != len(set(canonical_subjects)):
        raise TrialError("a canonical subject may be requested only once per command")
    declarations = evidence_index(trial)
    capture_state = state.get("captures")
    if not isinstance(capture_state, dict):
        raise TrialError("trial-state captures are malformed")
    intervention_detail_digest = None
    current_intervention = state.get("intervention", {}).get("status")
    if intervention_status is not None:
        if captures or canonical_subjects:
            raise TrialError(
                "record the intervention boundary separately from evidence capture"
            )
        if current_intervention != "not_performed":
            raise TrialError("intervention boundary was already recorded")
        if intervention_status == "performed" and intervention_detail_file is None:
            raise TrialError(
                "a performed intervention requires --intervention-detail-file"
            )
        missing_before = [
            name
            for name, declaration in declarations.items()
            if declaration.get("required") is True
            and evidence_phase(trial, name) == "before"
            and name not in capture_state
        ]
        if intervention_status == "performed" and missing_before:
            raise TrialError(
                "capture required before evidence before the intervention boundary: "
                + ", ".join(missing_before)
            )
        operator_detail_digest = optional_detail_digest(intervention_detail_file)
        if trusted and intervention_status == "performed":
            source = manifest["trusted_capture"]["intervention_sources"][trial["id"]]
            event = trusted_observation(
                state,
                trial_dir,
                source=source,
                subject="intervention",
                phase="intervention",
                target_name="intervention.json",
                evidence_format="json",
            )
            intervention_detail_digest = event["sha256"]
        else:
            intervention_detail_digest = operator_detail_digest
    elif intervention_detail_file is not None:
        raise TrialError("--intervention-detail-file requires --intervention-status")
    expected_phase = "after" if current_intervention == "performed" else "before"
    requested: list[tuple[str, Path | None]] = [
        (*parse_capture(raw_capture),) for raw_capture in captures
    ] + [(name, None) for name in canonical_subjects]
    for name, source in requested:
        declaration = declarations.get(name)
        if declaration is None:
            raise TrialError(f"{name} is not declared evidence for {trial['id']}")
        phase = evidence_phase(trial, name)
        if phase != expected_phase:
            raise TrialError(
                f"{name} is {phase}-intervention evidence; current capture phase is "
                f"{expected_phase}"
            )
        if name in capture_state:
            raise TrialError(f"capture {name} already exists; use a new trial directory")
        evidence_format = declaration["format"]
        target_name = f"{name}{FORMAT_SUFFIX[evidence_format]}"
        target = trial_dir / RAW_DIR / target_name
        if trusted:
            capture_state[name] = trusted_observation(
                state,
                trial_dir,
                source=declaration["source"],
                subject=name,
                phase=phase,
                target_name=target_name,
                evidence_format=evidence_format,
            )
        else:
            assert source is not None
            if not source.is_file() or source.is_symlink():
                raise TrialError(
                    f"capture source for {name} is not a regular non-symlink file"
                )
            if target.is_symlink():
                raise TrialError(f"capture target for {name} must not be a symlink")
            if target.exists() and name not in capture_state:
                raise TrialError(f"unexpected existing capture target for {name}")
            if source.resolve() != target.resolve():
                shutil.copyfile(source, target)
            validate_capture(target, evidence_format)
            captured_bytes = target.read_bytes()
            capture_state[name] = {
                "file": target_name,
                "format": evidence_format,
                "captured_at": now(),
                "bytes": len(captured_bytes),
                "sha256": digest_bytes(captured_bytes),
                "provenance": "operator_supplied",
            }
    if intervention_status is not None:
        state["intervention"] = {
            "kind": trial["intervention"]["kind"],
            "status": intervention_status,
            "recorded_at": now(),
            "detail_sha256": intervention_detail_digest,
            "operator_detail_sha256": operator_detail_digest,
        }
    save_state(trial_dir, state)
    print(trial_dir)


def command_cleanup(
    manifest_path: Path, trial_dir: Path, status: str, detail_file: Path | None
) -> None:
    manifest, trials = load_manifest(manifest_path)
    state = load_state(trial_dir)
    trial = trials[state["trial_id"]]
    current = state.get("cleanup", {}).get("status")
    if current != "not_run":
        raise TrialError("cleanup boundary was already recorded")
    if status == "completed" and detail_file is None:
        raise TrialError("completed cleanup requires --detail-file")
    operator_detail_digest = optional_detail_digest(detail_file)
    detail_sha256 = operator_detail_digest
    if state.get("capture_mode") == "trusted" and status == "completed":
        declarations = evidence_index(trial)
        missing_after = [
            name
            for name, declaration in declarations.items()
            if declaration.get("required") is True
            and evidence_phase(trial, name) == "after"
            and name not in state.get("captures", {})
        ]
        if missing_after:
            raise TrialError(
                "capture required after evidence before cleanup: "
                + ", ".join(missing_after)
            )
        event = trusted_observation(
            state,
            trial_dir,
            source=manifest["trusted_capture"]["cleanup_source"],
            subject="cleanup",
            phase="cleanup",
            target_name="cleanup.json",
            evidence_format="json",
        )
        detail_sha256 = event["sha256"]
    state["cleanup"] = {
        "status": status,
        "recorded_at": now(),
        "detail_sha256": detail_sha256,
        "operator_detail_sha256": operator_detail_digest,
    }
    save_state(trial_dir, state)
    print(trial_dir)


def json_pointer(value: Any, pointer: str) -> Any:
    if pointer == "":
        return value
    if not pointer.startswith("/"):
        raise TrialError("JSON pointer must be empty or start with /")
    current = value
    for encoded in pointer[1:].split("/"):
        token = encoded.replace("~1", "/").replace("~0", "~")
        if isinstance(current, dict) and token in current:
            current = current[token]
        elif isinstance(current, list) and token.isdigit():
            index = int(token)
            if index >= len(current):
                raise TrialError("JSON pointer array index is absent")
            current = current[index]
        else:
            raise TrialError("JSON pointer value is absent")
    return current


def load_captured_value(
    trial_dir: Path, state: dict[str, Any], name: str
) -> Any:
    capture = state.get("captures", {}).get(name)
    if not isinstance(capture, dict):
        raise TrialError(f"required evidence {name} was not captured")
    path = captured_file_path(trial_dir, capture)
    evidence_format = capture["format"]
    if evidence_format == "json":
        return load_json(path)
    if evidence_format == "jsonl":
        events = []
        for raw in path.read_text(encoding="utf-8").splitlines():
            if raw.strip():
                events.append(
                    json.loads(
                        raw,
                        parse_constant=lambda token: (_ for _ in ()).throw(
                            ValueError(f"non-finite JSON number {token}")
                        ),
                    )
                )
        return events
    return path.read_text(encoding="utf-8")


def captured_file_path(trial_dir: Path, capture: dict[str, Any]) -> Path:
    filename = capture.get("file")
    if (
        not isinstance(filename, str)
        or Path(filename).name != filename
        or re.fullmatch(r"[a-z][a-z0-9-]*\.(json|jsonl|txt)", filename) is None
    ):
        raise TrialError("capture state contains an unsafe evidence filename")
    path = trial_dir / RAW_DIR / filename
    if not path.is_file() or path.is_symlink():
        raise TrialError("captured evidence must remain a regular non-symlink file")
    data = path.read_bytes()
    if capture.get("bytes") != len(data) or capture.get("sha256") != digest_bytes(data):
        raise TrialError("captured evidence changed after its observation boundary")
    return path


def reference_value(
    trial_dir: Path, state: dict[str, Any], reference: dict[str, Any]
) -> Any:
    evidence = reference.get("evidence")
    pointer = reference.get("pointer")
    if not isinstance(evidence, str) or not isinstance(pointer, str):
        raise TrialError("oracle reference requires evidence and pointer")
    return json_pointer(load_captured_value(trial_dir, state, evidence), pointer)


def closed_object_fields(
    value: Any,
    required: set[str],
    optional: set[str],
) -> bool:
    return (
        isinstance(value, dict)
        and required.issubset(value)
        and set(value).issubset(required | optional)
    )


def valid_hex_signature(value: Any) -> bool:
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value) is not None


def valid_uuid(value: Any) -> bool:
    if not isinstance(value, str):
        return False
    try:
        parsed = uuid.UUID(value)
    except ValueError:
        return False
    return str(parsed) == value.lower()


def load_trusted_sandbox_boundary_observation(
    trial_dir: Path,
    state: dict[str, Any],
) -> tuple[dict[str, Any], bytes]:
    if state.get("capture_mode") != "trusted":
        raise TrialError("sandbox boundary observation lacks trusted capture provenance")
    trusted_capture = state.get("trusted_capture")
    if not isinstance(trusted_capture, dict) or trusted_capture.get("backend") not in {
        "sandbox-exec",
        "bwrap",
        "docker",
    }:
        raise TrialError("sandbox boundary observation lacks a contained capture binding")
    intervention = state.get("intervention")
    if not isinstance(intervention, dict) or intervention.get("status") != "performed":
        raise TrialError("sandbox boundary intervention was not performed")
    path = trial_dir / RAW_DIR / "intervention.json"
    raw = stable_regular_bytes(path, "trusted sandbox boundary observation")
    if intervention.get("detail_sha256") != digest_bytes(raw):
        raise TrialError("sandbox boundary observation changed after trusted capture")
    try:
        observation = json.loads(
            raw,
            parse_constant=lambda token: (_ for _ in ()).throw(
                ValueError(f"non-finite JSON number {token}")
            ),
        )
    except (UnicodeDecodeError, ValueError) as error:
        raise TrialError("sandbox boundary observation is malformed") from error
    if not isinstance(observation, dict):
        raise TrialError("sandbox boundary observation is not a JSON object")
    return observation, raw


def sandbox_boundary_observation_facts(
    trial_dir: Path,
    state: dict[str, Any],
    declaration: dict[str, Any],
    sandbox_boundary_probe_sha256: str | None,
) -> tuple[bool, dict[str, Any]]:
    observation, observation_raw = load_trusted_sandbox_boundary_observation(trial_dir, state)
    authority_name = declaration.get("authority_evidence")
    job_name = declaration.get("job_evidence")
    events_name = declaration.get("events_evidence")
    report_name = declaration.get("report_evidence")
    if not all(
        isinstance(name, str)
        for name in (authority_name, job_name, events_name, report_name)
    ):
        raise TrialError("sandbox boundary oracle lacks its canonical evidence declarations")
    authority = load_captured_value(trial_dir, state, authority_name)
    job_view = load_captured_value(trial_dir, state, job_name)
    events = load_captured_value(trial_dir, state, events_name)
    report = load_captured_value(trial_dir, state, report_name)
    authority_capture = state.get("captures", {}).get(authority_name)
    if not isinstance(authority_capture, dict):
        raise TrialError("sandbox boundary authority capture state is absent")
    authority_path = captured_file_path(trial_dir, authority_capture)

    observation_closed = closed_object_fields(
        observation,
        SANDBOX_BOUNDARY_OBSERVATION_REQUIRED_FIELDS,
        SANDBOX_BOUNDARY_OBSERVATION_OPTIONAL_FIELDS,
    )
    authority_closed = closed_object_fields(
        authority,
        JOB_AUTHORITY_REQUIRED_FIELDS,
        JOB_AUTHORITY_OPTIONAL_FIELDS,
    )
    trusted_capture = state["trusted_capture"]
    expected_backend = declaration.get("backend", trusted_capture.get("backend"))
    declared_backend = trusted_capture.get("backend")

    job_id = None
    projection = None
    attempts = None
    try:
        job_id = job_id_from_view(job_view)
        projection = json_pointer(job_view, "/job/projection")
        attempts = json_pointer(job_view, "/job/attempts")
    except TrialError:
        pass
    receipt = None
    try:
        receipt = json_pointer(report, "/receipt/receipt")
    except TrialError:
        pass

    run_id = observation.get("run_id")
    attempt = observation.get("attempt")
    outer_launch_id = observation.get("outer_launch_id")
    matching_attempts = []
    if isinstance(attempts, list):
        matching_attempts = [
            item
            for item in attempts
            if isinstance(item, dict)
            and item.get("id", {}).get("run_id") == run_id
        ]
    attempt_backends = {
        item.get("sandbox", {}).get("backend")
        for item in matching_attempts
        if isinstance(item.get("sandbox"), dict)
    }

    started = []
    linked = []
    if isinstance(events, list):
        started = [
            event
            for event in events
            if isinstance(event, dict)
            and event.get("kind") == "attempt_started"
            and isinstance(event.get("detail"), dict)
            and event["detail"].get("attempt") == attempt
            and event["detail"].get("run_id") == run_id
        ]
        linked = [
            event
            for event in events
            if isinstance(event, dict)
            and event.get("kind") == "child_linked"
            and isinstance(event.get("detail"), dict)
            and event["detail"].get("attempt") == attempt
            and event["detail"].get("run_id") == run_id
        ]
    linked_launch_ids = {
        event["detail"].get("launch_id")
        for event in linked
        if isinstance(event["detail"].get("launch_id"), str)
    }

    authority_identity = (
        authority.get("gate_evaluator_sha256") if isinstance(authority, dict) else None
    )
    observation_identity = observation.get("gate_evaluator_sha256")
    # These live trials exercise the current strict Job path, not the legacy
    # compatibility shape where evaluator identity was absent.  Requiring the
    # digest on both sides prevents a pair of missing values from being
    # mistaken for an independently bound evaluator.
    identity_valid = (
        isinstance(authority_identity, str)
        and valid_digest(authority_identity)
        and authority_identity == observation_identity
    )
    digests_valid = all(
        valid_digest(observation.get(name))
        for name in (
            "authority_sha256",
            "contract_sha256",
            "result_tree_sha256",
            "probe_sha256",
        )
    )
    observed_at_valid = False
    observed_at = observation.get("observed_at")
    if isinstance(observed_at, str):
        try:
            observed_at_valid = datetime.datetime.fromisoformat(
                observed_at.replace("Z", "+00:00")
            ).tzinfo is not None
        except ValueError:
            pass

    requested_backend = authority.get("sandbox_requested")
    backend_bound = (
        expected_backend in {"sandbox-exec", "bwrap", "docker"}
        and declared_backend == expected_backend
        and observation.get("sandbox_backend") == expected_backend
        and requested_backend in {"auto", expected_backend}
        # RunView records the approved request (`auto` or an explicit
        # backend), while the controller observation and receipt record the
        # concrete backend selected for the gate.
        and attempt_backends == {requested_backend}
    )
    if declaration.get("backend") is not None:
        backend_bound = backend_bound and requested_backend == expected_backend

    facts = {
        "observation_schema_closed": observation_closed,
        "authority_schema_closed": authority_closed,
        "trusted_intervention_digest_bound": True,
        "signature_present": valid_hex_signature(observation.get("signature")),
        "issuer_valid": observation.get("issuer") == "deadreckon-controller",
        "schema_version_valid": observation.get("schema_version") == 1,
        "timestamp_valid": observed_at_valid,
        "probe_identity_valid": (
            valid_uuid(observation.get("probe_id"))
            and valid_digest(sandbox_boundary_probe_sha256)
            and observation.get("probe_sha256") == sandbox_boundary_probe_sha256
        ),
        "authority_bound": (
            authority_closed
            and observation.get("authority_sha256")
            == digest_bytes(authority_path.read_bytes())
            and observation.get("job_id") == authority.get("job_id") == job_id
            and run_id == authority.get("run_id")
            and observation.get("contract_sha256") == authority.get("contract_sha256")
            and observation.get("sandbox_requested") == authority.get("sandbox_requested")
        ),
        "attempt_bound": (
            isinstance(attempt, int)
            and not isinstance(attempt, bool)
            and attempt > 0
            and isinstance(projection, dict)
            and projection.get("attempt_count") == attempt
            and len(matching_attempts) == 1
            and bool(started)
            and linked_launch_ids == {outer_launch_id}
            and valid_uuid(outer_launch_id)
        ),
        "backend_bound": backend_bound,
        "evaluator_identity_bound": identity_valid,
        "digests_well_formed": digests_valid,
        "completion_receipt_bound": (
            isinstance(receipt, dict)
            and receipt.get("job_id") == observation.get("job_id")
            and receipt.get("run_id") == run_id
            and receipt.get("authority_sha256") == observation.get("authority_sha256")
            and receipt.get("contract_sha256") == observation.get("contract_sha256")
            and receipt.get("result_tree_sha256") == observation.get("result_tree_sha256")
            and receipt.get("sandbox_backend") == observation.get("sandbox_backend")
            and receipt.get("contained") is True
            and receipt.get("sandbox_boundary_observation_sha256")
            == digest_bytes(observation_raw)
        ),
        "all_denials_observed": all(
            observation.get(field) is True for field in SANDBOX_BOUNDARY_DENIAL_FIELDS
        ),
    }
    return all(facts.values()), facts


def event_indices(events: list[dict[str, Any]], kind: str) -> list[int]:
    return [index for index, event in enumerate(events) if event.get("kind") == kind]


def parent_event_kind(event: Any) -> str | None:
    """Read both Campaign's flat event shape and Plan's nested wire shape."""
    if not isinstance(event, dict):
        return None
    kind = event.get("kind")
    if isinstance(kind, str):
        return kind
    nested = event.get("event")
    if isinstance(nested, dict) and isinstance(nested.get("kind"), str):
        return nested["kind"]
    return None


def event_suffix(
    trial_dir: Path,
    state: dict[str, Any],
    before_evidence: str,
    after_evidence: str,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    before = load_captured_value(trial_dir, state, before_evidence)
    after = load_captured_value(trial_dir, state, after_evidence)
    if not isinstance(before, list) or not isinstance(after, list):
        raise TrialError("event history evidence must be JSON Lines")
    if len(after) < len(before) or after[: len(before)] != before:
        raise TrialError("after event history is not an exact append of before history")
    return after, after[len(before) :]


def required_string(value: Any, pointer: str) -> str:
    observed = json_pointer(value, pointer)
    if not isinstance(observed, str) or not observed.strip():
        raise TrialError(f"{pointer} must be a non-empty string")
    return observed


def required_nonnegative_number(value: Any, pointer: str) -> float:
    observed = json_pointer(value, pointer)
    if (
        not isinstance(observed, (int, float))
        or isinstance(observed, bool)
        or not math.isfinite(observed)
        or observed < 0
    ):
        raise TrialError(f"{pointer} must be a non-negative number")
    return float(observed)


def event_detail_matches(event: dict[str, Any], expected: dict[str, Any]) -> bool:
    detail = event.get("detail")
    return isinstance(detail, dict) and all(detail.get(key) == value for key, value in expected.items())


def supervised_identity(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise TrialError("supervised child evidence must be one JSON object")
    identity: dict[str, Any] = {}
    for field in SUPERVISED_IDENTITY_FIELDS:
        observed = value.get(field)
        if field in {"pid", "attempt"}:
            if not isinstance(observed, int) or isinstance(observed, bool) or observed <= 0:
                raise TrialError(f"supervised child {field} must be a positive integer")
        elif not isinstance(observed, str) or not observed.strip():
            raise TrialError(f"supervised child {field} must be a non-empty string")
        identity[field] = observed
    owner_launch_id = value.get("owner_launch_id")
    if owner_launch_id is not None and (
        not isinstance(owner_launch_id, str) or not owner_launch_id.strip()
    ):
        raise TrialError("supervised child owner_launch_id must be null or non-empty")
    identity["owner_launch_id"] = owner_launch_id
    return identity


def job_id_from_view(value: Any) -> str:
    return required_string(value, "/job/job/job_id")


def job_report_integrity(report: Any, job_view: Any) -> dict[str, Any]:
    if not isinstance(report, dict):
        raise TrialError("Job report evidence must be one JSON object")
    job_id = job_id_from_view(job_view)
    report_id = required_string(report, "/id")
    if report_id != job_id:
        raise TrialError("Job report belongs to a different Job")
    report_phase = json_pointer(report, "/phase")
    report_outcome = json_pointer(report, "/outcome")
    report_stop_reason = json_pointer(report, "/stop_reason")
    view_phase = json_pointer(job_view, "/job/projection/phase")
    view_outcome = json_pointer(job_view, "/job/projection/outcome")
    view_stop_reason = json_pointer(job_view, "/job/projection/stop_reason")
    if [report_phase, report_outcome, report_stop_reason] != [
        view_phase,
        view_outcome,
        view_stop_reason,
    ]:
        raise TrialError("Job report lifecycle disagrees with the captured JobView")
    report_attempt_count = required_nonnegative_number(
        report, "/lifecycle/attempt_count"
    )
    report_lease_epoch = required_nonnegative_number(report, "/lifecycle/lease_epoch")
    report_last_sequence = required_nonnegative_number(
        report, "/lifecycle/last_event_sequence"
    )
    view_attempt_count = required_nonnegative_number(
        job_view, "/job/projection/attempt_count"
    )
    view_lease_epoch = required_nonnegative_number(
        job_view, "/job/projection/current_lease_epoch"
    )
    view_last_sequence = required_nonnegative_number(
        job_view, "/job/projection/last_sequence"
    )
    if (
        report_attempt_count != view_attempt_count
        or report_lease_epoch != view_lease_epoch
        or report_last_sequence != view_last_sequence
    ):
        raise TrialError("Job report counters disagree with the captured JobView")
    receipt_status = required_string(report, "/receipt/status")
    if report_outcome == "verified":
        if receipt_status != "valid":
            raise TrialError("verified Job report does not contain a valid receipt")
    elif receipt_status != "absent":
        raise TrialError("a non-verified Job report unexpectedly contains a receipt")
    if receipt_status == "valid":
        receipt_job_id = required_string(report, "/receipt/receipt/job_id")
        if receipt_job_id != job_id:
            raise TrialError("receipt belongs to a different Job")
    return {
        "job_id": job_id,
        "phase": report_phase,
        "outcome": report_outcome,
        "receipt_status": receipt_status,
        "attempt_count": report_attempt_count,
        "lease_epoch": report_lease_epoch,
        "last_event_sequence": report_last_sequence,
    }


def job_report_policy(report: Any, job_view: Any) -> dict[str, Any]:
    integrity = job_report_integrity(report, job_view)
    attempt_count = required_nonnegative_number(report, "/lifecycle/attempt_count")
    max_attempts = required_nonnegative_number(report, "/resources/max_attempts")
    recorded_spend = required_nonnegative_number(report, "/resources/recorded_spend_usd")
    max_spend = required_nonnegative_number(report, "/resources/max_spend_usd")
    elapsed = json_pointer(report, "/resources/lifecycle_elapsed_secs")
    max_wall = required_nonnegative_number(report, "/resources/max_wall_seconds")
    if elapsed is not None and (
        not isinstance(elapsed, (int, float))
        or isinstance(elapsed, bool)
        or not math.isfinite(elapsed)
        or elapsed < 0
    ):
        raise TrialError("lifecycle elapsed seconds must be null or non-negative")
    if attempt_count > max_attempts:
        raise TrialError("Job report exceeds its approved attempt limit")
    if recorded_spend > max_spend:
        raise TrialError("Job report exceeds its approved spend limit")
    if elapsed is not None and float(elapsed) > max_wall:
        raise TrialError("Job report exceeds its approved wall-time limit")
    return {
        **integrity,
        "attempt_count": attempt_count,
        "max_attempts": max_attempts,
        "recorded_spend_usd": recorded_spend,
        "max_spend_usd": max_spend,
        "lifecycle_elapsed_secs": elapsed,
        "max_wall_seconds": max_wall,
    }


def preserved_parent_work(before: Any, after: Any) -> dict[str, Any]:
    if not isinstance(before, dict) or not isinstance(after, dict):
        raise TrialError("parent artifacts must be JSON objects")
    if "campaign_id" in before:
        if before.get("campaign_id") != after.get("campaign_id"):
            raise TrialError("Campaign identity changed")
        collection_name, id_name = "sub_goals", "sub_id"
        completed_statuses = {"merged"}
    elif "plan_id" in before:
        if before.get("plan_id") != after.get("plan_id"):
            raise TrialError("Plan identity changed")
        collection_name, id_name = "tasks", "task_id"
        completed_statuses = {"done", "completed", "merged"}
    else:
        raise TrialError("artifact is neither a repository Plan nor Campaign")
    before_items = before.get(collection_name)
    after_items = after.get(collection_name)
    if not isinstance(before_items, list) or not isinstance(after_items, list):
        raise TrialError(f"{collection_name} must be an array")
    def index_children(items: list[Any], label: str) -> dict[str, dict[str, Any]]:
        indexed: dict[str, dict[str, Any]] = {}
        for item in items:
            if not isinstance(item, dict) or not isinstance(item.get(id_name), str):
                raise TrialError(f"{collection_name} contains a malformed identity")
            identity = item[id_name]
            if identity in indexed:
                raise TrialError(f"{label} {collection_name} repeats child {identity}")
            indexed[identity] = item
        return indexed

    indexed_before = index_children(before_items, "before")
    indexed_after = index_children(after_items, "after")
    if set(indexed_before) != set(indexed_after):
        raise TrialError("the parent child identity set changed")
    preserved = 0
    for prior in before_items:
        current = indexed_after.get(prior[id_name])
        assert current is not None
        for field in (
            "sub_plan_id",
            "subplan",
            "result_run_id",
            "child_run_id",
            "run_id",
        ):
            if prior.get(field) is not None and current.get(field) != prior.get(field):
                raise TrialError(f"persisted child field {field} changed")
        if prior.get("status") in completed_statuses:
            if current.get("status") != prior.get("status"):
                raise TrialError("completed child work was reopened")
            preserved += 1
    return {
        "artifact_kind": "campaign" if collection_name == "sub_goals" else "plan",
        "children": len(before_items),
        "completed_preserved": preserved,
    }


def terminal_projection(trial_dir: Path, state: dict[str, Any], name: str) -> bool:
    value = load_captured_value(trial_dir, state, name)
    try:
        phase = json_pointer(value, "/job/projection/phase")
    except TrialError:
        return False
    return phase == "terminal"


def oracle_result(
    declaration: dict[str, Any],
    status: str,
    reason: str,
    observed: Any | None,
) -> dict[str, Any]:
    return {
        "id": declaration["id"],
        "description": declaration["description"],
        "status": status,
        "reason": reason,
        "observed_sha256": digest_value(observed) if observed is not None else None,
    }


def evaluate_oracle(
    trial_dir: Path,
    state: dict[str, Any],
    declaration: dict[str, Any],
    *,
    sandbox_boundary_probe_sha256: str | None = None,
) -> dict[str, Any]:
    oracle_type = declaration.get("type")
    try:
        if oracle_type in {"json_equals", "json_not_equals"}:
            observed = reference_value(
                trial_dir,
                state,
                {
                    "evidence": declaration["evidence"],
                    "pointer": declaration["pointer"],
                },
            )
            matches = observed == declaration.get("expected")
            passed = matches if oracle_type == "json_equals" else not matches
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "captured value satisfied the declaration"
                if passed
                else "captured value contradicted the declaration",
                observed,
            )
        if oracle_type in {"json_values_equal", "json_values_not_equal"}:
            left = reference_value(trial_dir, state, declaration["left"])
            right = reference_value(trial_dir, state, declaration["right"])
            matches = left == right
            passed = matches if oracle_type == "json_values_equal" else not matches
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "captured values satisfied the declared relationship"
                if passed
                else "captured values contradicted the declared relationship",
                [left, right],
            )
        if oracle_type == "number_increased":
            before = reference_value(trial_dir, state, declaration["before"])
            after = reference_value(trial_dir, state, declaration["after"])
            if (
                not isinstance(before, (int, float))
                or isinstance(before, bool)
                or not math.isfinite(before)
                or not isinstance(after, (int, float))
                or isinstance(after, bool)
                or not math.isfinite(after)
            ):
                raise TrialError("number_increased values must be numeric")
            passed = after > before
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "captured number increased"
                if passed
                else "captured number did not increase",
                [before, after],
            )
        if oracle_type == "event_suffix_count":
            _, suffix = event_suffix(
                trial_dir,
                state,
                str(declaration["before_evidence"]),
                str(declaration["after_evidence"]),
            )
            count = len(event_indices(suffix, str(declaration["kind"])))
            minimum = int(declaration.get("minimum", 0))
            maximum = declaration.get("maximum")
            passed = count >= minimum and (
                maximum is None or count <= int(maximum)
            )
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "only newly appended events satisfied the declared count"
                if passed
                else "newly appended events were outside the declared count",
                {"suffix_events": len(suffix), "count": count},
            )
        if oracle_type == "event_suffix_order":
            _, suffix = event_suffix(
                trial_dir,
                state,
                str(declaration["before_evidence"]),
                str(declaration["after_evidence"]),
            )
            before = event_indices(suffix, str(declaration["before"]))
            after = event_indices(suffix, str(declaration["after"]))
            passed = bool(before and after and min(before) < min(after))
            if (
                before
                and not after
                and declaration.get("allow_missing_after_if_terminal") is True
                and terminal_projection(
                    trial_dir, state, str(declaration["terminal_evidence"])
                )
            ):
                passed = True
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "newly appended event order satisfied the declaration"
                if passed
                else "newly appended event order did not satisfy the declaration",
                {
                    "suffix_events": len(suffix),
                    "before_count": len(before),
                    "after_count": len(after),
                },
            )
        if oracle_type == "event_boundary_transition":
            full, suffix = event_suffix(
                trial_dir,
                state,
                str(declaration["before_evidence"]),
                str(declaration["after_evidence"]),
            )
            prefix = full[: len(full) - len(suffix)]
            before_kind = str(declaration["before"])
            after_kind = str(declaration["after"])
            before_indices = event_indices(prefix, before_kind)
            after_indices = event_indices(suffix, after_kind)
            semantic_kinds = {
                "semantic_judge_achieved",
                "semantic_judge_revise",
                "semantic_judge_uncertain",
            }
            later_semantic = [
                event
                for event in prefix[
                    (max(before_indices) + 1) if before_indices else len(prefix) :
                ]
                if event.get("kind") in semantic_kinds
            ]
            passed = bool(before_indices and after_indices and not later_semantic)
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "the pre-intervention decision transitions to a newly appended decision"
                if passed
                else "the semantic transition is stale or absent across the capture boundary",
                {
                    "before_count": len(before_indices),
                    "suffix_after_count": len(after_indices),
                    "later_prefix_semantic": len(later_semantic),
                },
            )
        if oracle_type == "job_report_integrity":
            report = load_captured_value(
                trial_dir, state, str(declaration["report_evidence"])
            )
            view = load_captured_value(
                trial_dir, state, str(declaration["job_evidence"])
            )
            observed = job_report_integrity(report, view)
            return oracle_result(
                declaration,
                "passed",
                "public report and any verified receipt are bound to the captured Job",
                observed,
            )
        if oracle_type == "job_event_history_bound":
            events = load_captured_value(
                trial_dir, state, str(declaration["evidence"])
            )
            view = load_captured_value(
                trial_dir, state, str(declaration["job_evidence"])
            )
            if not isinstance(events, list) or not events:
                raise TrialError("complete Job history must contain JSON Lines events")
            job_id = job_id_from_view(view)
            sequences: list[int] = []
            event_ids: list[str] = []
            for item in events:
                if not isinstance(item, dict) or item.get("job_id") != job_id:
                    raise TrialError("Job history contains a foreign or malformed event")
                sequence = item.get("sequence")
                event_id = item.get("event_id")
                if (
                    not isinstance(sequence, int)
                    or isinstance(sequence, bool)
                    or sequence <= 0
                    or not isinstance(event_id, str)
                    or not event_id
                ):
                    raise TrialError("Job history event identity is malformed")
                sequences.append(sequence)
                event_ids.append(event_id)
            passed = (
                sequences == list(range(1, len(sequences) + 1))
                and len(set(event_ids)) == len(event_ids)
            )
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "the complete event history is contiguous and bound to this Job"
                if passed
                else "the event history has gaps, duplicates, or conflicting order",
                {
                    "events": len(events),
                    "last_sequence": sequences[-1],
                    "unique_event_ids": len(set(event_ids)),
                },
            )
        if oracle_type == "job_report_within_policy":
            report = load_captured_value(
                trial_dir, state, str(declaration["report_evidence"])
            )
            view = load_captured_value(
                trial_dir, state, str(declaration["job_evidence"])
            )
            observed = job_report_policy(report, view)
            return oracle_result(
                declaration,
                "passed",
                "public report remains inside approved attempt, spend and wall limits",
                observed,
            )
        if oracle_type == "worker_target_stopped":
            child = supervised_identity(
                load_captured_value(
                    trial_dir, state, str(declaration["child_evidence"])
                )
            )
            view = load_captured_value(
                trial_dir, state, str(declaration["job_evidence"])
            )
            full, suffix = event_suffix(
                trial_dir,
                state,
                str(declaration["before_evidence"]),
                str(declaration["after_evidence"]),
            )
            job_id = job_id_from_view(view)
            matching_links = [
                event
                for event in full[: len(full) - len(suffix)]
                if event.get("kind") == "child_linked"
                and event.get("job_id") == job_id
                and event_detail_matches(
                    event,
                    {
                        "pid": child["pid"],
                        "launch_id": child["launch_id"],
                        "attempt": child["attempt"],
                        "release_token_sha256": child["release_token_sha256"],
                        "boot_id": child["boot_id"],
                        "process_start_identity": child["process_start_identity"],
                    },
                )
            ]
            matching_link = matching_links[0] if len(matching_links) == 1 else None
            stopped = [
                index
                for index, event in enumerate(suffix)
                if matching_link is not None
                and event.get("kind") == "attempt_stopped"
                and event.get("job_id") == job_id
                and event.get("lease_epoch") == matching_link.get("lease_epoch")
                and isinstance(event.get("detail"), dict)
                and event["detail"].get("attempt") == child["attempt"]
            ]
            restarted = [
                index
                for index, event in enumerate(suffix)
                if event.get("kind") == "attempt_started"
                and event.get("job_id") == job_id
                and isinstance(event.get("detail"), dict)
                and isinstance(event["detail"].get("attempt"), int)
                and event["detail"]["attempt"] > child["attempt"]
            ]
            passed = bool(
                matching_link
                and stopped
                and (not restarted or min(stopped) < min(restarted))
            )
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "the captured live launch is linked before the fault and stops before replacement"
                if passed
                else "the stopped attempt is not bound to the captured live launch",
                {
                    "matching_links": len(matching_links),
                    "matching_attempt_stops_in_suffix": len(stopped),
                    "restarted_in_suffix": len(restarted),
                },
            )
        if oracle_type == "lease_reclaim_bound":
            lease_before = load_captured_value(
                trial_dir, state, str(declaration["lease_before"])
            )
            lease_after = load_captured_value(
                trial_dir, state, str(declaration["lease_after"])
            )
            view = load_captured_value(
                trial_dir, state, str(declaration["job_evidence"])
            )
            _, suffix = event_suffix(
                trial_dir,
                state,
                str(declaration["before_evidence"]),
                str(declaration["after_evidence"]),
            )
            job_id = job_id_from_view(view)
            before_job = required_string(lease_before, "/job_id")
            after_job = required_string(lease_after, "/job_id")
            before_owner = required_string(lease_before, "/owner_id")
            after_owner = required_string(lease_after, "/owner_id")
            before_epoch = required_nonnegative_number(lease_before, "/epoch")
            after_epoch = required_nonnegative_number(lease_after, "/epoch")
            after_pid = json_pointer(lease_after, "/pid")
            after_boot = required_string(lease_after, "/boot_id")
            expected = {
                "job_id": job_id,
                "owner_id": after_owner,
                "epoch": int(after_epoch),
                "pid": after_pid,
                "boot_id": after_boot,
            }
            matches = [
                event
                for event in suffix
                if event.get("kind") == "lease_reclaimed"
                and event.get("job_id") == job_id
                and event.get("lease_epoch") == int(after_epoch)
                and event_detail_matches(event, expected)
            ]
            passed = bool(
                before_job == job_id
                and after_job == job_id
                and before_owner != after_owner
                and after_epoch > before_epoch
                and matches
            )
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "the appended reclaim is bound to a new owner and the same Job"
                if passed
                else "lease evidence does not prove this Job was reclaimed by a new owner",
                {
                    "same_job": before_job == after_job == job_id,
                    "owner_changed": before_owner != after_owner,
                    "epoch_increased": after_epoch > before_epoch,
                    "matching_reclaims": len(matches),
                },
            )
        if oracle_type == "child_adoption_bound":
            child_before = supervised_identity(
                load_captured_value(
                    trial_dir, state, str(declaration["child_before"])
                )
            )
            child_after = supervised_identity(
                load_captured_value(
                    trial_dir, state, str(declaration["child_after"])
                )
            )
            view = load_captured_value(
                trial_dir, state, str(declaration["job_evidence"])
            )
            _, suffix = event_suffix(
                trial_dir,
                state,
                str(declaration["before_evidence"]),
                str(declaration["after_evidence"]),
            )
            job_id = job_id_from_view(view)
            expected = {
                "adopted": True,
                "pid": child_after["pid"],
                "launch_id": child_after["launch_id"],
                "attempt": child_after["attempt"],
                "release_token_sha256": child_after["release_token_sha256"],
                "boot_id": child_after["boot_id"],
                "process_start_identity": child_after["process_start_identity"],
            }
            matches = [
                event
                for event in suffix
                if event.get("kind") == "child_linked"
                and event.get("job_id") == job_id
                and event_detail_matches(event, expected)
            ]
            passed = child_before == child_after and bool(matches)
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "the complete supervised-process identity was adopted by the replacement"
                if passed
                else "PID equality alone did not prove child adoption",
                {
                    "identity_preserved": child_before == child_after,
                    "matching_adoptions": len(matches),
                },
            )
        if oracle_type == "text_values_not_equal":
            before = load_captured_value(
                trial_dir, state, str(declaration["before_evidence"])
            )
            after = load_captured_value(
                trial_dir, state, str(declaration["after_evidence"])
            )
            if not isinstance(before, str) or not isinstance(after, str):
                raise TrialError("text comparison requires text evidence")
            before = before.strip()
            after = after.strip()
            passed = bool(before and after and before != after)
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "independent host observations changed"
                if passed
                else "independent host observations did not prove a change",
                [before, after],
            )
        if oracle_type == "doctor_backend_available":
            doctor = load_captured_value(
                trial_dir, state, str(declaration["evidence"])
            )
            sandboxes = json_pointer(doctor, "/sandboxes")
            backend = str(declaration["backend"])
            if not isinstance(sandboxes, list):
                raise TrialError("doctor sandboxes must be an array")
            matches = [
                item
                for item in sandboxes
                if isinstance(item, dict)
                and item.get("backend") == backend
                and item.get("available") is True
            ]
            return oracle_result(
                declaration,
                "passed" if matches else "failed",
                "repository-produced doctor output reports the backend available"
                if matches
                else "doctor output does not report the backend available",
                {"backend": backend, "available_matches": len(matches)},
            )
        if oracle_type == "supervisor_service_active":
            service = load_captured_value(
                trial_dir, state, str(declaration["evidence"])
            )
            if not isinstance(service, dict):
                raise TrialError("supervisor service evidence must be one JSON object")
            manager = service.get("manager")
            checkpoint = service.get("checkpoint")
            current_boot = service.get("current_boot_id")
            authoritative = (
                service.get("schema_version") == 1
                and service.get("installed") == "current"
                and service.get("test_override") is False
                and service.get("boot_identity_source")
                in {"linux_procfs", "macos_sysctl"}
                and isinstance(current_boot, str)
                and bool(current_boot)
                and isinstance(checkpoint, dict)
                and checkpoint.get("boot_id") == current_boot
                and isinstance(checkpoint.get("generation"), int)
                and not isinstance(checkpoint.get("generation"), bool)
                and checkpoint.get("generation", 0) > 0
                and isinstance(checkpoint.get("pid"), int)
                and not isinstance(checkpoint.get("pid"), bool)
                and checkpoint.get("pid", 0) > 0
                and isinstance(checkpoint.get("instance_id"), str)
                and bool(checkpoint.get("instance_id"))
            )
            active = (
                manager == "launchd"
                and service.get("loaded") is True
                and service.get("active") is None
                and service.get("enabled") is None
            ) or (
                manager == "systemd"
                and service.get("loaded") is None
                and service.get("active") == "active"
                and service.get("enabled") == "enabled"
            )
            passed = authoritative and active
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "typed service evidence is active and bound to the authoritative current boot"
                if passed
                else "service evidence is inactive, overridden, or not bound to the current boot",
                {
                    "manager": manager,
                    "authoritative": authoritative,
                    "active": active,
                    "checkpoint_generation": checkpoint.get("generation")
                    if isinstance(checkpoint, dict)
                    else None,
                },
            )
        if oracle_type == "parent_work_preserved":
            before = load_captured_value(
                trial_dir, state, str(declaration["before_evidence"])
            )
            after = load_captured_value(
                trial_dir, state, str(declaration["after_evidence"])
            )
            observed = preserved_parent_work(before, after)
            if "job_evidence" in declaration:
                view = load_captured_value(
                    trial_dir, state, str(declaration["job_evidence"])
                )
                job_id = job_id_from_view(view)
                artifact_id = before.get("campaign_id", before.get("plan_id"))
                if artifact_id != job_id:
                    raise TrialError("parent artifact belongs to a different Job")
                observed["job_bound"] = True
            passed = observed["completed_preserved"] > 0
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "repository parent artifacts preserve completed child identities",
                observed,
            )
        if oracle_type == "parent_only_repair":
            artifact = load_captured_value(
                trial_dir, state, str(declaration["artifact_evidence"])
            )
            _, suffix = event_suffix(
                trial_dir,
                state,
                str(declaration["before_evidence"]),
                str(declaration["after_evidence"]),
            )
            if isinstance(artifact, dict) and "campaign_id" in artifact:
                forbidden = {"sub_launch_prepared", "sub_launched"}
                artifact_kind = "campaign"
            elif isinstance(artifact, dict) and "plan_id" in artifact:
                forbidden = {"task_started", "task_retrying", "task_run_discovered"}
                artifact_kind = "plan"
            else:
                raise TrialError("parent event history has no recognized parent artifact")
            relaunched = [
                item
                for item in suffix
                if parent_event_kind(item) in forbidden
            ]
            return oracle_result(
                declaration,
                "passed" if not relaunched else "failed",
                "new parent events contain no successful-child relaunch"
                if not relaunched
                else "parent repair appended child relaunch events",
                {
                    "artifact_kind": artifact_kind,
                    "suffix_events": len(suffix),
                    "child_relaunch_events": len(relaunched),
                },
            )
        if oracle_type == "parent_repair_bound":
            repair = load_captured_value(
                trial_dir, state, str(declaration["repair_evidence"])
            )
            report = load_captured_value(
                trial_dir, state, str(declaration["report_evidence"])
            )
            view = load_captured_value(
                trial_dir, state, str(declaration["job_evidence"])
            )
            artifact = load_captured_value(
                trial_dir, state, str(declaration["artifact_evidence"])
            )
            policy = job_report_policy(report, view)
            job_id = policy["job_id"]
            repair_job = required_string(repair, "/job_id")
            shape = required_string(repair, "/shape")
            round_number = required_nonnegative_number(repair, "/round")
            requested_after_attempt = required_nonnegative_number(
                repair, "/requested_after_attempt"
            )
            requested_epoch = required_nonnegative_number(
                repair, "/requested_after_lease_epoch"
            )
            report_epoch = required_nonnegative_number(report, "/lifecycle/lease_epoch")
            artifact_shape = (
                "campaign"
                if isinstance(artifact, dict) and "campaign_id" in artifact
                else "graph"
                if isinstance(artifact, dict) and "plan_id" in artifact
                else "unknown"
            )
            passed = bool(
                repair_job == job_id
                and shape in {"graph", "campaign"}
                and shape == artifact_shape
                and round_number >= 1
                and requested_after_attempt < policy["attempt_count"]
                and requested_epoch <= report_epoch
            )
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "the repair intent is same-Job, fenced, and inside public policy"
                if passed
                else "the repair intent is unbound, unfenced, or outside public policy",
                {
                    "same_job": repair_job == job_id,
                    "shape": shape,
                    "artifact_shape": artifact_shape,
                    "round": round_number,
                    "requested_after_attempt": requested_after_attempt,
                    "final_attempt_count": policy["attempt_count"],
                    "requested_lease_epoch": requested_epoch,
                    "final_lease_epoch": report_epoch,
                },
            )
        if oracle_type == "campaign_recovery_bound":
            campaign_before = load_captured_value(
                trial_dir, state, str(declaration["campaign_before"])
            )
            campaign_after = load_captured_value(
                trial_dir, state, str(declaration["campaign_after"])
            )
            plan_before = load_captured_value(
                trial_dir, state, str(declaration["plan_before"])
            )
            plan_after = load_captured_value(
                trial_dir, state, str(declaration["plan_after"])
            )
            _, suffix = event_suffix(
                trial_dir,
                state,
                str(declaration["events_before"]),
                str(declaration["events_after"]),
            )
            plan_events, plan_suffix = event_suffix(
                trial_dir,
                state,
                str(declaration["plan_events_before"]),
                str(declaration["plan_events_after"]),
            )
            campaign_observed = preserved_parent_work(campaign_before, campaign_after)
            plan_observed = preserved_parent_work(plan_before, plan_after)
            plan_id = plan_before.get("plan_id")
            campaign_plan_ids = {
                item.get("sub_plan_id")
                for item in campaign_before.get("sub_goals", [])
                if isinstance(item, dict)
            }
            if not isinstance(plan_id, str) or plan_id not in campaign_plan_ids:
                raise TrialError("active Plan is not linked by the Campaign artifact")
            if any(
                not isinstance(item, dict) or item.get("plan_id") != plan_id
                for item in plan_events
            ):
                raise TrialError("active Plan history contains a foreign Plan event")
            active_subs = [
                item
                for item in campaign_before.get("sub_goals", [])
                if isinstance(item, dict)
                and item.get("status") == "running"
                and item.get("sub_plan_id") == plan_id
            ]
            recovered = [
                item
                for item in suffix
                if item.get("kind") == "sub_recovered"
                and isinstance(item.get("detail"), dict)
                and any(
                    item["detail"].get("sub_id") == sub.get("sub_id")
                    and item["detail"].get("plan_id") == plan_id
                    for sub in active_subs
                )
            ]
            after_events = load_captured_value(
                trial_dir, state, str(declaration["events_after"])
            )
            seen: set[tuple[str, Any, Any]] = set()
            duplicates = 0
            for event in after_events:
                if event.get("kind") not in {"sub_launch_prepared", "sub_launched"}:
                    continue
                detail = event.get("detail")
                if not isinstance(detail, dict):
                    raise TrialError("Campaign launch event has malformed detail")
                key = (event["kind"], detail.get("sub_id"), detail.get("plan_id"))
                if None in key or key in seen:
                    duplicates += 1
                seen.add(key)
            completed_task_ids = {
                item.get("task_id")
                for item in plan_before.get("tasks", [])
                if isinstance(item, dict)
                and item.get("status") in {"done", "completed", "merged"}
                and isinstance(item.get("task_id"), str)
            }
            reopened_completed = 0
            for item in plan_suffix:
                nested = item.get("event") if isinstance(item, dict) else None
                if not isinstance(nested, dict):
                    raise TrialError("Plan event history has a malformed nested event")
                if (
                    nested.get("kind")
                    in {"task_started", "task_retrying", "task_run_discovered"}
                    and nested.get("task_id") in completed_task_ids
                ):
                    reopened_completed += 1
            passed = bool(
                duplicates == 0
                and reopened_completed == 0
                and active_subs
                and recovered
            )
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "real Campaign and Plan artifacts retain identities without duplicate launch facts"
                if passed
                else "Campaign artifacts or events contain duplicate persisted launch facts",
                {
                    "campaign": campaign_observed,
                    "plan": plan_observed,
                    "suffix_events": len(suffix),
                    "duplicate_launch_facts": duplicates,
                    "plan_suffix_events": len(plan_suffix),
                    "completed_plan_tasks_reopened": reopened_completed,
                    "active_subplans": len(active_subs),
                    "matching_recoveries": len(recovered),
                },
            )
        if oracle_type == "sandbox_boundary_observation_bound":
            try:
                passed, facts = sandbox_boundary_observation_facts(
                    trial_dir,
                    state,
                    declaration,
                    sandbox_boundary_probe_sha256,
                )
            except (KeyError, TypeError, TrialError, ValueError):
                return oracle_result(
                    declaration,
                    "failed",
                    "the trusted sandbox boundary observation was absent, malformed, or unbound",
                    None,
                )
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "the authenticated controller probe is bound to the exact Job, attempt, backend, evaluator, receipt, and denial facts"
                if passed
                else "the sandbox boundary observation contradicted at least one required binding or denial fact",
                facts,
            )
        if oracle_type == "structurally_inconclusive":
            return oracle_result(
                declaration,
                "inconclusive",
                "the repository has no authoritative producer for this live observation",
                None,
            )
        if oracle_type == "event_count":
            events = load_captured_value(
                trial_dir, state, str(declaration["evidence"])
            )
            if not isinstance(events, list):
                raise TrialError("event_count evidence must be JSON Lines")
            count = len(event_indices(events, str(declaration["kind"])))
            minimum = int(declaration.get("minimum", 0))
            maximum = declaration.get("maximum")
            passed = count >= minimum and (
                maximum is None or count <= int(maximum)
            )
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "event count was within declared bounds"
                if passed
                else "event count was outside declared bounds",
                {"count": count},
            )
        if oracle_type in {"event_before", "event_after"}:
            events = load_captured_value(
                trial_dir, state, str(declaration["evidence"])
            )
            if not isinstance(events, list):
                raise TrialError(f"{oracle_type} evidence must be JSON Lines")
            before = event_indices(events, str(declaration["before"]))
            after = event_indices(events, str(declaration["after"]))
            if before and after:
                passed = (
                    min(before) < min(after)
                    if oracle_type == "event_before"
                    else any(left < right for left in before for right in after)
                )
                return oracle_result(
                    declaration,
                    "passed" if passed else "failed",
                    "event order satisfied the declaration"
                    if passed
                    else "event order contradicted the declaration",
                    {"before_index": min(before), "after_index": min(after)},
                )
            if (
                before
                and not after
                and declaration.get("allow_missing_after_if_terminal") is True
                and terminal_projection(
                    trial_dir, state, str(declaration["terminal_evidence"])
                )
            ):
                return oracle_result(
                    declaration,
                    "passed",
                    "the earlier event exists and the Job stopped terminally without retry",
                    {"before_index": min(before), "terminal": True},
                )
            return oracle_result(
                declaration,
                "failed",
                "required ordered events were absent",
                {"before_count": len(before), "after_count": len(after)},
            )
        if oracle_type == "text_contains_any":
            text = load_captured_value(
                trial_dir, state, str(declaration["evidence"])
            )
            needles = declaration.get("needles")
            if not isinstance(text, str) or not isinstance(needles, list):
                raise TrialError("text_contains_any requires text evidence and needles")
            passed = any(
                isinstance(needle, str) and needle in text for needle in needles
            )
            return oracle_result(
                declaration,
                "passed" if passed else "failed",
                "captured text contained an accepted signal"
                if passed
                else "captured text omitted every accepted signal",
                {"text_sha256": digest_bytes(text.encode("utf-8"))},
            )
        raise TrialError(f"unsupported oracle type: {oracle_type}")
    except (KeyError, TypeError, ValueError, TrialError) as error:
        return oracle_result(
            declaration,
            "inconclusive",
            f"evidence could not evaluate this oracle: {type(error).__name__}",
            None,
        )


def job_prefix(trial_dir: Path, state: dict[str, Any]) -> str | None:
    for name in ("job-view-after", "job-view-offline", "job-view-before"):
        if name not in state.get("captures", {}):
            continue
        try:
            value = load_captured_value(trial_dir, state, name)
            job_id = str(json_pointer(value, "/job/job/job_id"))
        except TrialError:
            continue
        normalized = re.sub(r"[^A-Za-z0-9_-]", "", job_id)
        if len(normalized) >= 4:
            return normalized[:8]
    return None


def captured_backend(trial_dir: Path, state: dict[str, Any]) -> str:
    references = [
        ("job-report", "/receipt/sandbox_backend"),
        ("job-view-after", "/job/attempts/0/sandbox/backend"),
        ("job-view-offline", "/job/attempts/0/sandbox/backend"),
        ("job-view-before", "/job/attempts/0/sandbox/backend"),
    ]
    for name, pointer in references:
        if name not in state.get("captures", {}):
            continue
        try:
            value = json_pointer(
                load_captured_value(trial_dir, state, name), pointer
            )
        except TrialError:
            continue
        if isinstance(value, str) and value in ALLOWED_BACKENDS:
            return value
    return "unknown"


def sanitized_evidence(
    trial_dir: Path,
    state: dict[str, Any],
    declarations: dict[str, dict[str, Any]],
) -> list[dict[str, Any]]:
    records = []
    captures = state.get("captures", {})
    if not isinstance(captures, dict) or not set(captures).issubset(declarations):
        raise TrialError("capture state contains undeclared evidence")
    for name, capture in sorted(captures.items()):
        path = captured_file_path(trial_dir, capture)
        data = path.read_bytes()
        records.append(
            {
                "name": name,
                "declared_source": declarations[name]["source"],
                "sha256": digest_bytes(data),
                "bytes": len(data),
                "media_type": MEDIA_TYPE[capture["format"]],
            }
        )
    return records


def sanitized_lifecycle_record(
    state: dict[str, Any],
    key: str,
    allowed_statuses: set[str],
) -> dict[str, Any]:
    record = state.get(key)
    if not isinstance(record, dict) or record.get("status") not in allowed_statuses:
        raise TrialError(f"{key} state is malformed")
    recorded_at = record.get("recorded_at")
    if recorded_at is not None:
        if not isinstance(recorded_at, str):
            raise TrialError(f"{key} recorded_at is malformed")
        try:
            parsed = datetime.datetime.fromisoformat(recorded_at)
        except ValueError as error:
            raise TrialError(f"{key} recorded_at is malformed") from error
        if parsed.tzinfo is None:
            raise TrialError(f"{key} recorded_at must include a timezone")
    detail = record.get("detail_sha256")
    if detail is not None and not valid_digest(detail):
        raise TrialError(f"{key} detail digest is malformed")
    return {
        "status": record["status"],
        "recorded_at": recorded_at,
        "detail_sha256": detail,
    }


def result_status(
    state: dict[str, Any],
    assertions: list[dict[str, Any]],
    *,
    trusted_capture_verified: bool = False,
) -> str:
    intervention_record = state["intervention"]
    intervention = intervention_record["status"]
    captures = state.get("captures", {})
    if intervention == "not_performed" and not captures:
        return "not_run"
    if intervention != "performed" or not valid_digest(
        intervention_record.get("detail_sha256")
    ):
        return "inconclusive"
    statuses = {assertion["status"] for assertion in assertions}
    if "failed" in statuses:
        return "failed"
    if "inconclusive" in statuses:
        return "inconclusive"
    # The trial directory and its state are operator-writable documentation.
    # Never infer trusted provenance from those bytes. The private capture
    # helper must authenticate its protected receipt during finalization and
    # explicitly supply this process-local decision.
    if not trusted_capture_verified:
        return "inconclusive"
    cleanup_record = state.get("cleanup", {})
    cleanup = cleanup_record.get("status")
    if cleanup == "failed":
        return "failed"
    if cleanup != "completed" or not valid_digest(
        cleanup_record.get("detail_sha256")
    ):
        return "inconclusive"
    return "passed"


def valid_digest(value: Any) -> bool:
    return (
        isinstance(value, str)
        and re.fullmatch(r"sha256:[0-9a-f]{64}", value) is not None
    )


EVALUATION_FIELDS = {
    "schema_version",
    "generated_at",
    "sanitized",
    "trial_id",
    "status",
    "source_revision",
    "host",
    "backend",
    "provider_slots",
    "job_id_prefix",
    "capture_provenance",
    "intervention",
    "oracle_assertions",
    "evidence",
    "cleanup",
    "limitations",
}


def validate_lifecycle_payload(payload: dict[str, Any]) -> tuple[
    dict[str, Any], dict[str, Any], list[Any]
]:
    if payload["backend"] not in ALLOWED_BACKENDS:
        raise TrialError("result payload contains an unrecognized backend")
    intervention = payload.get("intervention")
    cleanup = payload.get("cleanup")
    assertions = payload.get("oracle_assertions")
    if (
        not isinstance(intervention, dict)
        or not isinstance(intervention.get("kind"), str)
        or not intervention["kind"]
        or not isinstance(cleanup, dict)
        or not isinstance(assertions, list)
        or not assertions
    ):
        raise TrialError("result payload has malformed lifecycle evidence")
    return intervention, cleanup, assertions


def validate_evaluation_payload(payload: dict[str, Any]) -> None:
    if set(payload) != EVALUATION_FIELDS:
        raise TrialError("evaluation does not match the closed pre-seal contract")
    if payload["schema_version"] != 1 or payload["sanitized"] is not True:
        raise TrialError("evaluation has an invalid schema or sanitization marker")
    provenance = payload.get("capture_provenance")
    if (
        not isinstance(provenance, dict)
        or set(provenance) != {"status", "receipt_sha256"}
        or provenance.get("status")
        not in {"operator_attested", "trusted_preseal"}
        or provenance.get("receipt_sha256") is not None
    ):
        raise TrialError("evaluation has malformed pre-seal capture provenance")
    intervention, cleanup, assertions = validate_lifecycle_payload(payload)
    if payload["status"] == "passed":
        if (
            provenance.get("status") != "trusted_preseal"
            or payload["backend"] in {"none", "unknown"}
            or intervention.get("status") != "performed"
            or not valid_digest(intervention.get("detail_sha256"))
            or cleanup.get("status") != "completed"
            or not valid_digest(cleanup.get("detail_sha256"))
            or any(assertion.get("status") != "passed" for assertion in assertions)
        ):
            raise TrialError("passed evaluation violates the fail-closed contract")


def validate_result_payload(payload: dict[str, Any]) -> None:
    required = {
        "schema_version",
        "sanitized",
        "evaluation",
        "evaluation_sha256",
        "capture_provenance",
    }
    if set(payload) != required:
        raise TrialError("result envelope does not match the closed result contract")
    if payload["schema_version"] != 2 or payload["sanitized"] is not True:
        raise TrialError("result envelope has an invalid schema or sanitization marker")
    if not valid_digest(payload.get("evaluation_sha256")):
        raise TrialError("result envelope has no valid evaluation digest")
    evaluation = payload.get("evaluation")
    if not isinstance(evaluation, dict):
        raise TrialError("result envelope has no embedded evaluation")
    validate_evaluation_payload(evaluation)
    if digest_bytes(pretty_json_bytes(evaluation)) != payload["evaluation_sha256"]:
        raise TrialError("embedded evaluation does not match its bound digest")
    provenance = payload.get("capture_provenance")
    if (
        not isinstance(provenance, dict)
        or set(provenance)
        != {"status", "receipt_sha256", "publication_proof"}
        or provenance.get("status") not in {"operator_attested", "verified"}
        or (
            provenance.get("status") == "verified"
            and (
                not valid_digest(provenance.get("receipt_sha256"))
                or not isinstance(provenance.get("publication_proof"), str)
                or re.fullmatch(
                    r"[0-9a-f]{64}",
                    provenance["publication_proof"],
                )
                is None
            )
        )
        or (
            provenance.get("status") == "operator_attested"
            and (
                provenance.get("receipt_sha256") is not None
                or provenance.get("publication_proof") is not None
            )
        )
    ):
        raise TrialError("result envelope has malformed capture provenance")
    intervention, cleanup, assertions = validate_lifecycle_payload(evaluation)
    if evaluation["status"] == "passed":
        if (
            provenance.get("status") != "verified"
            or not valid_digest(provenance.get("receipt_sha256"))
            or evaluation["backend"] in {"none", "unknown"}
            or intervention.get("status") != "performed"
            or not valid_digest(intervention.get("detail_sha256"))
            or cleanup.get("status") != "completed"
            or not valid_digest(cleanup.get("detail_sha256"))
            or any(assertion.get("status") != "passed" for assertion in assertions)
        ):
            raise TrialError("passed result violates the fail-closed result contract")


def trusted_capture_inspect(
    state: dict[str, Any],
    trial_dir: Path,
    expected_intervention_source: str,
) -> dict[str, Any]:
    response = run_capture_helper(
        state,
        ["inspect", *helper_identity(state)],
    )
    capture = state["trusted_capture"]
    if (
        response.get("verified") is not True
        or response.get("job_id") != capture["job_id"]
        or response.get("session_id") != state["session_id"]
        or response.get("trial_id") != state["trial_id"]
    ):
        raise TrialError("trusted capture inspection returned a mismatched identity")
    coverage = response.get("capture_coverage")
    subjects = response.get("subject_coverage")
    if not isinstance(coverage, dict) or not isinstance(subjects, list):
        raise TrialError("trusted capture inspection omitted capture coverage")
    for name, record in state.get("captures", {}).items():
        matches = [
            event
            for event in subjects
            if isinstance(event, dict)
            and event.get("subject") == name
            and normalized_helper_name(event.get("source")) == record.get("source")
            and normalized_helper_name(event.get("phase")) == record.get("phase")
        ]
        if len(matches) != 1 or matches[0].get("content_sha256") != record.get(
            "sha256"
        ) or matches[0].get("content_bytes") != record.get("bytes"):
            raise TrialError(
                f"trusted capture inspection does not cover canonical subject {name}"
            )
    for key, subject, phase, source, kind in (
        (
            "intervention",
            "intervention",
            "intervention",
            expected_intervention_source,
            "intervention-recorded",
        ),
        ("cleanup", "cleanup", "cleanup", "job-cleanup", "cleanup-recorded"),
    ):
        lifecycle = state.get(key, {})
        if lifecycle.get("status") not in {"performed", "completed"}:
            continue
        matches = [
            event
            for event in subjects
            if isinstance(event, dict)
            and event.get("subject") == subject
            and normalized_helper_name(event.get("phase")) == phase
            and normalized_helper_name(event.get("source")) == source
            and normalized_helper_name(event.get("kind")) == kind
            and normalized_helper_name(event.get("provenance")) == "trusted-supervisor"
            and event.get("content_sha256") == lifecycle.get("detail_sha256")
        ]
        evidence_path = trial_dir / RAW_DIR / f"{subject}.json"
        evidence = stable_regular_bytes(evidence_path, f"trusted {subject} observation")
        if (
            len(matches) != 1
            or matches[0].get("content_bytes") != len(evidence)
            or matches[0].get("content_sha256") != digest_bytes(evidence)
        ):
            raise TrialError(f"trusted capture inspection does not cover {key}")
    return response


def build_evaluation(
    manifest_path: Path,
    trial_dir: Path,
    state: dict[str, Any],
    *,
    trusted_pass_ready: bool,
    generated_at: str | None = None,
) -> dict[str, Any]:
    manifest, trials = load_manifest(manifest_path)
    trial = trials[state["trial_id"]]
    declarations = evidence_index(trial)
    assertions = [
        evaluate_oracle(
            trial_dir,
            state,
            declaration,
            sandbox_boundary_probe_sha256=manifest["trusted_capture"].get(
                "sandbox_boundary_probe_sha256"
            ),
        )
        for declaration in trial["oracles"]
    ]
    missing_required = [
        name
        for name, declaration in declarations.items()
        if declaration.get("required") is True
        and name not in state.get("captures", {})
    ]
    for name in missing_required:
        assertions.append(
            {
                "id": f"required_evidence_{name}",
                "description": f"Required evidence {name} was captured.",
                "status": "inconclusive",
                "reason": "required evidence was absent",
                "observed_sha256": None,
            }
        )
    intervention_record = sanitized_lifecycle_record(
        state,
        "intervention",
        {"not_performed", "performed", "failed"},
    )
    cleanup_record = sanitized_lifecycle_record(
        state,
        "cleanup",
        {"not_run", "completed", "failed"},
    )
    evaluation = {
        "schema_version": 1,
        "generated_at": generated_at or now(),
        "sanitized": True,
        "trial_id": trial["id"],
        "status": result_status(
            state,
            assertions,
            trusted_capture_verified=trusted_pass_ready,
        ),
        "source_revision": validate_revision(state["source_revision"]),
        "host": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
        "backend": captured_backend(trial_dir, state),
        "provider_slots": list(trial["job"]["provider_slots"]),
        "job_id_prefix": job_prefix(trial_dir, state),
        "capture_provenance": {
            "status": (
                "trusted_preseal"
                if state.get("capture_mode") == "trusted"
                else "operator_attested"
            ),
            "receipt_sha256": None,
        },
        "intervention": {
            **intervention_record,
            "kind": trial["intervention"]["kind"],
        },
        "oracle_assertions": assertions,
        "evidence": sanitized_evidence(trial_dir, state, declarations),
        "cleanup": {
            **cleanup_record,
            "instructions": list(trial["cleanup"]),
        },
        "limitations": list(trial.get("limitations", [])),
    }
    validate_evaluation_payload(evaluation)
    return evaluation


def command_evaluate_bundle(
    manifest_path: Path,
    trial_dir: Path,
    template_path: Path,
    output: Path,
) -> None:
    state = load_state(trial_dir)
    if state.get("capture_mode") != "trusted":
        raise TrialError("trusted evaluation bundle has invalid capture mode")
    template = load_json(template_path)
    validate_evaluation_payload(template)
    evaluation = build_evaluation(
        manifest_path,
        trial_dir,
        state,
        trusted_pass_ready=True,
        generated_at=template["generated_at"],
    )
    write_json_no_clobber(output, evaluation)
    print(output)


def command_finalize(
    manifest_path: Path,
    trial_dir: Path,
    output: Path,
    evaluation_output: Path | None = None,
) -> None:
    state = load_state(trial_dir)
    trusted = state.get("capture_mode") == "trusted"
    manifest, _ = load_manifest(manifest_path)
    expected_intervention_source = manifest["trusted_capture"][
        "intervention_sources"
    ].get(state.get("trial_id"))
    if trusted and expected_intervention_source not in TRUSTED_INTERVENTION_SOURCES:
        raise TrialError("trusted capture manifest has no intervention source for this trial")
    inspection = (
        trusted_capture_inspect(state, trial_dir, expected_intervention_source)
        if trusted
        else None
    )
    trusted_pass_ready = bool(
        inspection
        and inspection.get("capture_coverage", {}).get("pass_ready") is True
    )
    evaluation = build_evaluation(
        manifest_path,
        trial_dir,
        state,
        trusted_pass_ready=trusted_pass_ready,
    )
    if evaluation_output is None:
        evaluation_output = output.with_name(f"{output.stem}.evaluation.json")
    if evaluation_output.resolve(strict=False) == output.resolve(strict=False):
        raise TrialError("evaluation and result envelope require distinct paths")
    if evaluation_output.exists() or evaluation_output.is_symlink():
        if not evaluation_output.is_file() or evaluation_output.is_symlink():
            raise TrialError("existing evaluation must be a regular non-symlink file")
        existing_evaluation_bytes = stable_regular_bytes(
            evaluation_output, "existing immutable evaluation"
        )
        try:
            existing_evaluation = json.loads(existing_evaluation_bytes)
        except ValueError as error:
            raise TrialError("existing immutable evaluation is malformed") from error
        if not isinstance(existing_evaluation, dict):
            raise TrialError("existing immutable evaluation must be a JSON object")
        existing_generated_at = existing_evaluation.get("generated_at")
        if not isinstance(existing_generated_at, str):
            raise TrialError("existing evaluation generated_at is malformed")
        evaluation["generated_at"] = existing_generated_at
        if (
            existing_evaluation != evaluation
            or existing_evaluation_bytes != pretty_json_bytes(existing_evaluation)
        ):
            raise TrialError("existing immutable evaluation conflicts with current state")
        evaluation = existing_evaluation
    else:
        write_json_no_clobber(evaluation_output, evaluation)
    evaluation_bytes = stable_regular_bytes(
        evaluation_output, "immutable evaluation"
    )
    evaluation_sha256 = digest_bytes(evaluation_bytes)
    receipt_sha256 = None
    publication_proof = None
    provenance_status = "operator_attested"
    if trusted:
        run_capture_helper(
            state,
            [
                "seal",
                *helper_identity(state),
                "--result",
                str(evaluation_output),
                "--status",
                evaluation["status"],
            ],
        )
        verdict = run_capture_helper(
            state,
            [
                "verify",
                *helper_identity(state),
                "--result",
                str(evaluation_output),
            ],
        )
        capture = state["trusted_capture"]
        binding_coverage = verdict.get("binding_coverage")
        if (
            verdict.get("verified") is not True
            or verdict.get("job_id") != capture["job_id"]
            or verdict.get("session_id") != state["session_id"]
            or verdict.get("trial_id") != state["trial_id"]
            or normalized_helper_name(verdict.get("status")) != evaluation["status"]
            or not valid_digest(verdict.get("receipt_sha256"))
            or not isinstance(binding_coverage, dict)
            or not binding_coverage
            or not all(value is True for value in binding_coverage.values())
        ):
            raise TrialError("trusted capture verification failed closed")
        receipt_sha256 = verdict["receipt_sha256"]
        publication_proof = verdict.get("publication_proof")
        if (
            not isinstance(publication_proof, str)
            or re.fullmatch(r"[0-9a-f]{64}", publication_proof) is None
        ):
            raise TrialError("trusted capture verification omitted publication proof")
        provenance_status = "verified"
    payload = {
        "schema_version": 2,
        "sanitized": True,
        "evaluation": evaluation,
        "evaluation_sha256": evaluation_sha256,
        "capture_provenance": {
            "status": provenance_status,
            "receipt_sha256": receipt_sha256,
            "publication_proof": publication_proof,
        },
    }
    validate_result_payload(payload)
    if output.exists() or output.is_symlink():
        if not output.is_file() or output.is_symlink():
            raise TrialError("existing result envelope must be a regular non-symlink file")
        existing_payload_bytes = stable_regular_bytes(
            output, "existing immutable result envelope"
        )
        try:
            existing_payload = json.loads(existing_payload_bytes)
        except ValueError as error:
            raise TrialError("existing immutable result envelope is malformed") from error
        if not isinstance(existing_payload, dict):
            raise TrialError("existing immutable result envelope must be a JSON object")
        validate_result_payload(existing_payload)
        if (
            existing_payload != payload
            or existing_payload_bytes != pretty_json_bytes(payload)
        ):
            raise TrialError("existing immutable result envelope conflicts with verification")
    else:
        write_json_no_clobber(output, payload)
    if trusted:
        run_capture_helper(
            state,
            [
                "verify",
                *helper_identity(state),
                "--result",
                str(evaluation_output),
                "--envelope",
                str(output),
            ],
        )
    print(output)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Passively record operator-run Watchkeeper live fault trials."
    )
    parser.add_argument(
        "--manifest",
        type=Path,
        default=DEFAULT_MANIFEST,
        help="live trial manifest (defaults beside this script)",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    prepare = subparsers.add_parser("prepare")
    prepare.add_argument("trial_id", choices=sorted(EXPECTED_CLAIM_IDS))
    prepare.add_argument("--trial-dir", type=Path, required=True)
    prepare.add_argument("--revision", required=True)
    prepare.add_argument("--capture-helper", type=Path)
    prepare.add_argument("--deadreckon-binary", type=Path)
    prepare.add_argument("--job-id")
    prepare.add_argument(
        "--backend",
        choices=["sandbox-exec", "bwrap", "docker"],
    )
    prepare.add_argument("--provider-route", action="append", default=[])

    observe = subparsers.add_parser("observe")
    observe.add_argument("--trial-dir", type=Path, required=True)
    observe.add_argument("--capture", action="append", default=[])
    observe.add_argument("--canonical", action="append", default=[])
    observe.add_argument("--replace", action="store_true")
    observe.add_argument(
        "--intervention-status",
        choices=["not_performed", "performed", "failed"],
    )
    observe.add_argument("--intervention-detail-file", type=Path)

    finalize = subparsers.add_parser("finalize")
    finalize.add_argument("--trial-dir", type=Path, required=True)
    finalize.add_argument("--output", type=Path, required=True)
    finalize.add_argument("--evaluation-output", type=Path)

    cleanup = subparsers.add_parser("cleanup")
    cleanup.add_argument("--trial-dir", type=Path, required=True)
    cleanup.add_argument("--status", choices=["completed", "failed"], required=True)
    cleanup.add_argument("--detail-file", type=Path)

    evaluate_bundle = subparsers.add_parser(
        "evaluate-bundle",
        help=argparse.SUPPRESS,
    )
    evaluate_bundle.add_argument("--trial-dir", type=Path, required=True)
    evaluate_bundle.add_argument("--template", type=Path, required=True)
    evaluate_bundle.add_argument("--output", type=Path, required=True)
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    try:
        if args.command == "prepare":
            command_prepare(
                args.manifest,
                args.trial_id,
                args.trial_dir,
                args.revision,
                capture_helper=args.capture_helper,
                deadreckon_binary=args.deadreckon_binary,
                backend=args.backend,
                provider_routes=args.provider_route,
                job_id=args.job_id,
            )
        elif args.command == "observe":
            command_observe(
                args.manifest,
                args.trial_dir,
                args.capture,
                args.canonical,
                args.replace,
                args.intervention_status,
                args.intervention_detail_file,
            )
        elif args.command == "finalize":
            command_finalize(
                args.manifest,
                args.trial_dir,
                args.output,
                args.evaluation_output,
            )
        elif args.command == "cleanup":
            command_cleanup(
                args.manifest,
                args.trial_dir,
                args.status,
                args.detail_file,
            )
        elif args.command == "evaluate-bundle":
            command_evaluate_bundle(
                args.manifest,
                args.trial_dir,
                args.template,
                args.output,
            )
        else:
            parser.error("unknown command")
    except TrialError as error:
        print(f"live trial recorder: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
