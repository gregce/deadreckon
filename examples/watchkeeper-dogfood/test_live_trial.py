#!/usr/bin/env python3
"""Adversarial tests for the passive Watchkeeper live fault-trial recorder."""

from __future__ import annotations

import ast
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent
SCRIPT = ROOT / "live-trial.py"
MANIFEST = ROOT / "live-trials.json"
SCHEMA = ROOT / "live-trial-results.schema.json"
EXPECTED_IDS = {
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
REVISION = "defd889cfa7c2b21ea77b3053026157c02612fc4"
JOB_ID = "01234567-89ab-cdef-0123-456789abcdef"

SPEC = importlib.util.spec_from_file_location("watchkeeper_live_trial", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
RECORDER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RECORDER)


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def write_jsonl(path: Path, values: list[dict[str, object]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "".join(json.dumps(value) + "\n" for value in values),
        encoding="utf-8",
    )


def run_recorder(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), "--manifest", str(MANIFEST), *args],
        check=check,
        capture_output=True,
        text=True,
    )


def job_view(
    job_id: str = JOB_ID,
    *,
    backend: str = "sandbox-exec",
    provider: str = "cli:worker",
    phase: str = "running",
    outcome: str | None = None,
    stop_reason: str | None = None,
    attempt_count: int = 1,
    lease_epoch: int = 2,
    last_sequence: int = 6,
) -> dict:
    return {
        "kind": "job_status",
        "job": {
            "job": {"job_id": job_id},
            "projection": {
                "phase": phase,
                "outcome": outcome,
                "stop_reason": stop_reason,
                "attempt_count": attempt_count,
                "current_lease_epoch": lease_epoch,
                "last_sequence": last_sequence,
            },
            "attempts": [
                {
                    "id": {
                        "scope": "watchkeeper-test",
                        "run_id": job_id,
                        "short": job_id[:8],
                    },
                    "provider": provider,
                    "sandbox": {"backend": backend},
                }
            ],
            "missing_attempts": [],
        },
    }


def job_report(
    job_id: str = JOB_ID,
    *,
    backend: str | None = None,
    phase: str = "running",
    outcome: str | None = None,
    stop_reason: str | None = None,
    attempt_count: int = 1,
    max_attempts: int = 3,
    lease_epoch: int = 2,
    last_sequence: int = 6,
) -> dict:
    verified = outcome == "verified"
    return {
        "id": job_id,
        "phase": phase,
        "outcome": outcome,
        "stop_reason": stop_reason,
        "lifecycle": {
            "attempt_count": attempt_count,
            "lease_epoch": lease_epoch,
            "last_event_sequence": last_sequence,
        },
        "resources": {
            "recorded_spend_usd": 1.0,
            "max_spend_usd": 4.0,
            "lifecycle_elapsed_secs": 20,
            "max_wall_seconds": 120,
            "max_attempts": max_attempts,
        },
        "receipt": {
            "status": "valid" if verified else "absent",
            "contained": True if verified else None,
            "sandbox_backend": backend,
            "receipt": {"job_id": job_id} if verified else None,
        },
    }


def supervised_child() -> dict:
    return {
        "schema_version": 1,
        "pid": 300,
        "pgid": 300,
        "launch_id": "launch-a",
        "attempt": 1,
        "owner_launch_id": "owner-a",
        "release_token_sha256": "sha256:release",
        "boot_id": "boot-a",
        "process_start_identity": "start-a",
        "phase": "running",
    }


def lease(*, owner: str, epoch: int, pid: int) -> dict:
    return {
        "schema_version": 1,
        "job_id": JOB_ID,
        "owner_id": owner,
        "epoch": epoch,
        "boot_id": "boot-a",
        "pid": pid,
        "process_group": pid,
        "acquired_at": "2026-07-30T00:00:00Z",
        "heartbeat_at": "2026-07-30T00:00:00Z",
        "expires_at": "2026-07-30T00:00:15Z",
    }


def event(
    sequence: int,
    kind: str,
    *,
    detail: dict | None = None,
    lease_epoch: int = 1,
) -> dict:
    return {
        "schema_version": 1,
        "job_id": JOB_ID,
        "sequence": sequence,
        "event_id": f"event-{sequence}",
        "causation_id": f"cause-{sequence}",
        "timestamp": f"2026-07-30T00:00:{sequence:02d}Z",
        "lease_epoch": lease_epoch,
        "kind": kind,
        "detail": detail or {},
    }


def supervisor_histories() -> tuple[list[dict], list[dict]]:
    child = supervised_child()
    before = [
        event(1, "created", lease_epoch=0),
        event(2, "lease_acquired", lease_epoch=1),
        event(3, "attempt_started", detail={"attempt": 1}),
        event(
            4,
            "child_linked",
            detail={
                "adopted": False,
                "pid": child["pid"],
                "launch_id": child["launch_id"],
                "attempt": child["attempt"],
                "release_token_sha256": child["release_token_sha256"],
                "boot_id": child["boot_id"],
                "process_start_identity": child["process_start_identity"],
            },
        ),
    ]
    lease_after = lease(owner="owner-b", epoch=2, pid=200)
    after = before + [
        event(
            5,
            "lease_reclaimed",
            lease_epoch=2,
            detail={
                "job_id": JOB_ID,
                "owner_id": lease_after["owner_id"],
                "epoch": lease_after["epoch"],
                "pid": lease_after["pid"],
                "boot_id": lease_after["boot_id"],
            },
        ),
        event(
            6,
            "child_linked",
            lease_epoch=2,
            detail={
                "adopted": True,
                "pid": child["pid"],
                "launch_id": child["launch_id"],
                "attempt": child["attempt"],
                "release_token_sha256": child["release_token_sha256"],
                "boot_id": child["boot_id"],
                "process_start_identity": child["process_start_identity"],
            },
        ),
    ]
    return before, after


class ManifestContractTests(unittest.TestCase):
    def test_manifest_covers_exactly_nine_claims_with_authoritative_references(self) -> None:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
        ids = [trial["id"] for trial in manifest["trials"]]
        supported_oracles = {
            "json_equals",
            "json_not_equals",
            "json_values_equal",
            "json_values_not_equal",
            "number_increased",
            "event_count",
            "event_before",
            "event_after",
            "event_suffix_count",
            "event_suffix_order",
            "event_boundary_transition",
            "job_event_history_bound",
            "job_report_integrity",
            "job_report_within_policy",
            "worker_target_stopped",
            "lease_reclaim_bound",
            "child_adoption_bound",
            "text_values_not_equal",
            "text_contains_any",
            "doctor_backend_available",
            "supervisor_service_active",
            "parent_work_preserved",
            "parent_only_repair",
            "parent_repair_bound",
            "campaign_recovery_bound",
            "sandbox_boundary_observation_bound",
            "structurally_inconclusive",
        }
        self.assertEqual(set(ids), EXPECTED_IDS)
        self.assertEqual(len(ids), len(EXPECTED_IDS))
        self.assertEqual(set(manifest["claim_ids"]), EXPECTED_IDS)
        self.assertEqual(manifest["trusted_capture"]["helper"], "dr-capture")
        self.assertEqual(
            manifest["trusted_capture"]["sandbox_boundary_probe_sha256"],
            "sha256:05d6c7c8e44cbd769e76beb24e60d5865236bd434f2cc2b0950f5d94e047a5dd",
        )
        self.assertEqual(
            manifest["trusted_capture"]["cleanup_source"], "job-cleanup"
        )
        self.assertEqual(
            set(manifest["trusted_capture"]["intervention_sources"]),
            EXPECTED_IDS,
        )
        for trial in manifest["trials"]:
            declarations = {item["name"]: item for item in trial["evidence"]}
            self.assertEqual(
                declarations,
                RECORDER.evidence_index(trial),
                trial["id"],
            )
            self.assertTrue(
                all(
                    item["source"] in RECORDER.ALLOWED_EVIDENCE_SOURCES
                    for item in declarations.values()
                ),
                trial["id"],
            )
            capture_phases = trial["capture_phases"]
            phased = capture_phases["before"] + capture_phases["after"]
            self.assertEqual(set(capture_phases), {"before", "after"}, trial["id"])
            self.assertEqual(len(phased), len(set(phased)), trial["id"])
            self.assertEqual(set(phased), set(declarations), trial["id"])
            self.assertTrue(trial["prerequisites"], trial["id"])
            self.assertTrue(trial["job"]["provider_slots"], trial["id"])
            self.assertTrue(trial["intervention"]["operator_only"], trial["id"])
            self.assertTrue(trial["intervention"]["instructions"], trial["id"])
            self.assertTrue(trial["oracles"], trial["id"])
            self.assertTrue(trial["cleanup"], trial["id"])
            self.assertTrue(
                trial["finalize"].startswith(
                    "python3 examples/watchkeeper-dogfood/live-trial.py"
                ),
                trial["id"],
            )
            for oracle in trial["oracles"]:
                self.assertIn(oracle["type"], supported_oracles, trial["id"])
                for key, value in oracle.items():
                    if (
                        isinstance(value, str)
                        and (
                            key == "evidence"
                            or key.endswith("_evidence")
                            or key
                            in {
                                "child_before",
                                "child_after",
                                "lease_before",
                                "lease_after",
                                "campaign_before",
                                "campaign_after",
                                "plan_before",
                                "plan_after",
                                "events_before",
                                "events_after",
                                "plan_events_before",
                                "plan_events_after",
                            }
                        )
                    ):
                        self.assertIn(value, declarations, (trial["id"], oracle["id"], key))
                    if key in {"left", "right", "before", "after"} and isinstance(
                        value, dict
                    ):
                        self.assertIn(
                            value["evidence"],
                            declarations,
                            (trial["id"], oracle["id"], key),
                        )
            for oracle in trial["oracles"]:
                if oracle["type"].startswith("event_suffix") or oracle[
                    "type"
                ] == "event_boundary_transition":
                    self.assertTrue(
                        declarations[oracle["before_evidence"]]["required"],
                        (trial["id"], oracle["id"]),
                    )

    def test_arbitrary_attack_and_recovery_summaries_are_not_declared(self) -> None:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
        evidence = {
            item["name"]
            for trial in manifest["trials"]
            for item in trial["evidence"]
        }
        self.assertNotIn("attack-observation", evidence)
        self.assertNotIn("campaign-recovery-summary", evidence)
        structurally_inconclusive = {
            trial["id"]
            for trial in manifest["trials"]
            if any(
                oracle["type"] == "structurally_inconclusive"
                for oracle in trial["oracles"]
            )
        }
        self.assertEqual(
            structurally_inconclusive,
            {
                "live_provider_network_loss",
                "live_campaign_interruption_recovery",
            },
        )

    def test_result_schema_is_closed_and_fail_closed_for_pass(self) -> None:
        schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
        properties = schema["properties"]
        self.assertEqual(set(properties["trial_id"]["enum"]), EXPECTED_IDS)
        self.assertEqual(
            set(properties["backend"]["enum"]),
            {"sandbox-exec", "bwrap", "docker", "none", "unknown"},
        )
        pass_then = schema["allOf"][0]["then"]["properties"]
        self.assertEqual(
            pass_then["capture_provenance"]["properties"]["status"]["const"],
            "trusted_preseal",
        )
        self.assertIsNone(
            pass_then["capture_provenance"]["properties"]["receipt_sha256"][
                "const"
            ]
        )
        self.assertEqual(
            set(pass_then["backend"]["enum"]),
            {"sandbox-exec", "bwrap", "docker"},
        )
        self.assertEqual(
            pass_then["intervention"]["properties"]["status"]["const"], "performed"
        )
        self.assertEqual(
            pass_then["cleanup"]["properties"]["status"]["const"], "completed"
        )
        self.assertEqual(
            pass_then["oracle_assertions"]["items"]["properties"]["status"]["const"],
            "passed",
        )
        self.assertFalse(schema["additionalProperties"])

    def test_recorder_only_executes_the_explicit_capture_helper(self) -> None:
        tree = ast.parse(SCRIPT.read_text(encoding="utf-8"))
        imported = {
            alias.name.split(".", 1)[0]
            for node in ast.walk(tree)
            if isinstance(node, (ast.Import, ast.ImportFrom))
            for alias in node.names
        }
        self.assertTrue(
            {"socket", "requests", "urllib", "signal"}.isdisjoint(imported)
        )
        self.assertIn("subprocess", imported)
        subprocess_calls = [
            node
            for node in ast.walk(tree)
            if isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id == "subprocess"
        ]
        self.assertEqual(len(subprocess_calls), 1)


class RecorderFlowTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.trial_dir = self.root / "trial"
        self.fixture_dir = self.root / "fixtures"
        self.output = self.root / "sanitized-result.json"
        self.fixture_dir.mkdir()
        self.fake_helper = self.root / "dr-capture"
        self.fake_deadreckon = self.root / "deadreckon"
        self.fake_deadreckon.write_text("fake deadreckon\n", encoding="utf-8")
        self.fake_helper.write_text(
            """#!/usr/bin/env python3
import hashlib
import json
import shutil
import sys
from pathlib import Path

base = Path(__file__).resolve().parent
state_path = base / "fake-capture-state.json"
log_path = base / "fake-capture-log.jsonl"
args = sys.argv[1:]
command = args[0]

def flag(name):
    return args[args.index(name) + 1]

def digest(data):
    return "sha256:" + hashlib.sha256(data).hexdigest()

with log_path.open("a", encoding="utf-8") as log:
    log.write(json.dumps(args) + "\\n")
state = json.loads(state_path.read_text()) if state_path.exists() else {"events": []}
job_id = flag("--job-id")
session_id = flag("--session-id")
if command == "prepare":
    state.update({
        "job_id": job_id,
        "session_id": session_id,
        "trial_id": flag("--trial-id"),
        "manifest": flag("--manifest"),
    })
    state_path.write_text(json.dumps(state))
    failpoint = base / "fail-prepare-once"
    if failpoint.exists():
        failpoint.unlink()
        raise SystemExit(19)
    print(json.dumps({
        "job_id": job_id,
        "session_id": session_id,
        "trial_id": state["trial_id"],
        "source_revision": "REVISION_TOKEN",
    }))
elif command == "observe":
    subject = flag("--subject")
    source = flag("--source")
    phase = flag("--phase")
    output = Path(flag("--output"))
    output.parent.mkdir(parents=True, exist_ok=True)
    fixture = base / "fixtures" / output.name
    if subject in {"intervention", "cleanup"}:
        data = json.dumps({"subject": subject, "source": source}, sort_keys=True).encode()
    else:
        data = fixture.read_bytes()
    if output.exists() and output.read_bytes() != data:
        raise SystemExit(9)
    if not output.exists():
        output.write_bytes(data)
    event = {
        "job_id": job_id,
        "session_id": session_id,
        "event_id": flag("--event-id"),
        "subject": subject,
        "source": source,
        "phase": phase,
        "kind": (
            "intervention_recorded" if subject == "intervention"
            else "cleanup_recorded" if subject == "cleanup"
            else "observation_recorded"
        ),
        "timestamp": "2026-07-30T12:00:00+00:00",
        "content_sha256": digest(data),
        "content_bytes": len(data),
        "provenance": "trusted_supervisor",
    }
    prior = [item for item in state["events"] if item["event_id"] == event["event_id"]]
    if prior and prior[0] != event:
        raise SystemExit(10)
    if not prior:
        state["events"].append(event)
    state_path.write_text(json.dumps(state))
    print(json.dumps(event))
elif command == "inspect":
    manifest = json.loads(Path(state["manifest"]).read_text())
    trial = next(item for item in manifest["trials"] if item["id"] == state["trial_id"])
    required = {item["name"] for item in trial["evidence"] if item["required"]}
    subjects = {item["subject"] for item in state["events"]}
    pass_ready = required <= subjects and {"intervention", "cleanup"} <= subjects
    print(json.dumps({
        "verified": True,
        "job_id": job_id,
        "session_id": session_id,
        "trial_id": state["trial_id"],
        "capture_coverage": {
            "required_total": len(required),
            "required_covered": len(required & subjects),
            "intervention_covered": "intervention" in subjects,
            "cleanup_covered": "cleanup" in subjects,
            "pass_ready": pass_ready,
            "missing": sorted(required - subjects),
        },
        "subject_coverage": state["events"],
    }))
elif command == "seal":
    result = Path(flag("--result")).read_bytes()
    state["status"] = flag("--status")
    state["result_sha256"] = digest(result)
    state_path.write_text(json.dumps(state))
    failpoint = base / "fail-seal-once"
    if failpoint.exists():
        failpoint.unlink()
        raise SystemExit(20)
    print(json.dumps({"status": state["status"]}))
elif command == "verify":
    result = Path(flag("--result")).read_bytes()
    if digest(result) != state["result_sha256"]:
        raise SystemExit(11)
    receipt_sha = digest(b"fake-protected-receipt")
    proof = "b" * 64
    failpoint = base / "fail-verify-once"
    if failpoint.exists():
        failpoint.unlink()
        raise SystemExit(21)
    if "--envelope" in args:
        envelope = json.loads(Path(flag("--envelope")).read_text())
        provenance = envelope["capture_provenance"]
        if (
            envelope["evaluation_sha256"] != state["result_sha256"]
            or provenance["receipt_sha256"] != receipt_sha
            or provenance["publication_proof"] != proof
        ):
            raise SystemExit(12)
        failpoint = base / "fail-envelope-verify-once"
        if failpoint.exists():
            failpoint.unlink()
            raise SystemExit(22)
    print(json.dumps({
        "verified": True,
        "job_id": job_id,
        "session_id": session_id,
        "trial_id": state["trial_id"],
        "status": state["status"],
        "receipt_sha256": receipt_sha,
        "publication_proof": proof,
        "binding_coverage": {
            "job_source": True,
            "deadreckon_source": True,
            "manifest": True,
            "result_schema": True,
            "recorder": True,
            "capture_binary": True,
            "deadreckon_binary": True,
            "execution_declaration": True,
            "replay": True
        },
        "capture_coverage": {"pass_ready": True},
        "subject_coverage": state["events"],
    }))
else:
    raise SystemExit(13)
""".replace("REVISION_TOKEN", REVISION),
            encoding="utf-8",
        )
        self.fake_helper.chmod(0o700)

    def tearDown(self) -> None:
        self.temp.cleanup()

    def prepare(
        self,
        trial_id: str = "live_provider_supervisor_restart",
        *,
        trusted: bool = False,
    ) -> None:
        trusted_args = (
            [
                "--capture-helper",
                str(self.fake_helper),
                "--deadreckon-binary",
                str(self.fake_deadreckon),
                "--job-id",
                JOB_ID,
                "--backend",
                "sandbox-exec",
                "--provider-route",
                "worker=cli:worker",
                "--provider-route",
                "independent_judge=cli:judge",
            ]
            if trusted
            else []
        )
        run_recorder(
            "prepare",
            trial_id,
            "--trial-dir",
            str(self.trial_dir),
            "--revision",
            REVISION,
            *trusted_args,
        )

    def detail(self, name: str, content: str = "operator evidence") -> Path:
        path = self.fixture_dir / name
        path.write_text(content + "\n", encoding="utf-8")
        return path

    def supervisor_captures(
        self,
        *,
        backend: str = "sandbox-exec",
        report_job_id: str = JOB_ID,
        stale_after: bool = False,
        report_last_sequence: int | None = None,
        nonverified_receipt: bool = False,
    ) -> tuple[list[str], list[str]]:
        before_events, after_events = supervisor_histories()
        if stale_after:
            stale_lease = lease(owner="owner-b", epoch=2, pid=200)
            old_reclaim = event(
                5,
                "lease_reclaimed",
                lease_epoch=2,
                detail={
                    "job_id": JOB_ID,
                    "owner_id": stale_lease["owner_id"],
                    "epoch": stale_lease["epoch"],
                    "pid": stale_lease["pid"],
                    "boot_id": stale_lease["boot_id"],
                },
            )
            old_adoption = event(
                6,
                "child_linked",
                lease_epoch=2,
                detail={
                    "adopted": True,
                    "pid": supervised_child()["pid"],
                    "launch_id": supervised_child()["launch_id"],
                    "attempt": supervised_child()["attempt"],
                    "release_token_sha256": supervised_child()[
                        "release_token_sha256"
                    ],
                    "boot_id": supervised_child()["boot_id"],
                    "process_start_identity": supervised_child()[
                        "process_start_identity"
                    ],
                },
            )
            before_events = before_events + [old_reclaim, old_adoption]
            after_events = before_events + [
                event(7, "deterministic_gate_passed", lease_epoch=2)
            ]
        report_value = job_report(
            report_job_id,
            lease_epoch=2,
            last_sequence=(
                len(after_events)
                if report_last_sequence is None
                else report_last_sequence
            ),
        )
        if nonverified_receipt:
            report_value["receipt"] = {
                "status": "valid",
                "contained": True,
                "sandbox_backend": backend,
                "receipt": {"job_id": report_job_id},
            }
        fixtures = {
            "job-view-before": job_view(
                backend=backend,
                lease_epoch=1,
                last_sequence=len(before_events),
            ),
            "job-view-after": job_view(
                backend=backend,
                lease_epoch=2,
                last_sequence=len(after_events),
            ),
            "lease-before": lease(owner="owner-a", epoch=1, pid=100),
            "lease-after": lease(owner="owner-b", epoch=2, pid=200),
            "supervised-child-before": supervised_child(),
            "supervised-child-after": supervised_child(),
            "job-report": report_value,
        }
        before_captures = []
        after_captures = []
        for name, value in fixtures.items():
            path = self.fixture_dir / f"{name}.json"
            write_json(path, value)
            target = before_captures if name.endswith("-before") else after_captures
            target.extend(["--capture", f"{name}={path}"])
        for name, values in {
            "events-before": before_events,
            "events-after": after_events,
        }.items():
            path = self.fixture_dir / f"{name}.jsonl"
            write_jsonl(path, values)
            target = before_captures if name.endswith("-before") else after_captures
            target.extend(["--capture", f"{name}={path}"])
        return before_captures, after_captures

    def observe_supervisor(self, **capture_options: object) -> None:
        before_captures, after_captures = self.supervisor_captures(
            **capture_options
        )
        run_recorder(
            "observe",
            "--trial-dir",
            str(self.trial_dir),
            *before_captures,
        )
        intervention = self.detail(
            "intervention.txt", "terminated owner-a pid 100 launch owner-a"
        )
        run_recorder(
            "observe",
            "--trial-dir",
            str(self.trial_dir),
            "--intervention-status",
            "performed",
            "--intervention-detail-file",
            str(intervention),
        )
        run_recorder(
            "observe",
            "--trial-dir",
            str(self.trial_dir),
            *after_captures,
        )

    def complete_cleanup(self) -> None:
        cleanup = self.detail("cleanup.txt", "Job terminal; disposable processes absent")
        run_recorder(
            "cleanup",
            "--trial-dir",
            str(self.trial_dir),
            "--status",
            "completed",
            "--detail-file",
            str(cleanup),
        )

    def finalize(self) -> dict:
        run_recorder(
            "finalize",
            "--trial-dir",
            str(self.trial_dir),
            "--output",
            str(self.output),
        )
        return json.loads(self.output.read_text(encoding="utf-8"))

    def test_prepare_writes_repo_root_replay_and_intervention_kind(self) -> None:
        self.prepare()
        state = json.loads(
            (self.trial_dir / "trial-state.json").read_text(encoding="utf-8")
        )
        replay = json.loads(
            (self.trial_dir / "replay.json").read_text(encoding="utf-8")
        )
        self.assertEqual(
            state["intervention"]["kind"], "terminate_live_supervisor"
        )
        self.assertTrue(
            replay["observe_command"].startswith(
                "python3 examples/watchkeeper-dogfood/live-trial.py"
            )
        )
        self.assertIn("events-before", replay["before_observe_command"])
        self.assertIn(
            "--intervention-detail-file", replay["intervention_record_command"]
        )
        self.assertIn("job-report", replay["after_observe_command"])
        self.assertIn("--detail-file", replay["cleanup_record_command"])

    def test_typed_service_oracle_requires_live_authoritative_boot_binding(self) -> None:
        raw = self.trial_dir / "raw"
        raw.mkdir(parents=True)
        path = raw / "service-before.json"
        service = {
            "schema_version": 1,
            "manager": "launchd",
            "installed": "current",
            "loaded": True,
            "enabled": None,
            "active": None,
            "checkpoint": {
                "schema_version": 1,
                "generation": 2,
                "instance_id": "service-instance",
                "boot_id": "boot-a",
                "pid": 4242,
            },
            "current_boot_id": "boot-a",
            "boot_identity_source": "macos_sysctl",
            "test_override": False,
        }
        write_json(path, service)

        def capture_state() -> dict:
            data = path.read_bytes()
            return {
                "captures": {
                    "service-before": {
                        "file": path.name,
                        "format": "json",
                        "captured_at": "2026-07-30T00:00:00+00:00",
                        "bytes": len(data),
                        "sha256": "sha256:" + hashlib.sha256(data).hexdigest(),
                        "provenance": "operator_supplied",
                    }
                }
            }

        declaration = {
            "id": "service_active",
            "description": "The service is active.",
            "type": "supervisor_service_active",
            "evidence": "service-before",
        }
        assertion = RECORDER.evaluate_oracle(
            self.trial_dir, capture_state(), declaration
        )
        self.assertEqual(assertion["status"], "passed")

        service["test_override"] = True
        service["boot_identity_source"] = "test_override"
        write_json(path, service)
        assertion = RECORDER.evaluate_oracle(
            self.trial_dir, capture_state(), declaration
        )
        self.assertEqual(assertion["status"], "failed")

    def sandbox_boundary_oracle_fixture(
        self,
        *,
        backend: str = "docker",
        requested_backend: str | None = None,
    ) -> tuple[dict, dict, dict, dict]:
        raw = self.trial_dir / "raw"
        raw.mkdir(parents=True, exist_ok=True)
        requested_backend = requested_backend or backend
        evaluator_identity = "sha256:" + ("e" * 64)
        authority = {
            "schema_version": 1,
            "job_id": JOB_ID,
            "run_id": JOB_ID,
            "approved_at": "2026-07-30T11:59:00Z",
            "accepted_by": "operator",
            "goal_sha256": "sha256:" + ("1" * 64),
            "contract_sha256": "sha256:" + ("2" * 64),
            "effective_policy_sha256": "sha256:" + ("3" * 64),
            "launch_plan_sha256": "sha256:" + ("4" * 64),
            "source_tree_sha256": "sha256:" + ("5" * 64),
            "source_revision": REVISION,
            "sandbox_requested": requested_backend,
            "semantic_judge_mode": "required",
            "gate_evaluator_sha256": evaluator_identity,
        }
        authority_path = raw / "authority.json"
        write_json(authority_path, authority)
        launch_id = "76ebebba-6d43-4c43-aaf4-690a6bd7ad6c"
        observation = {
            "schema_version": 1,
            "job_id": JOB_ID,
            "run_id": JOB_ID,
            "observed_at": "2026-07-30T12:00:00Z",
            "issuer": "deadreckon-controller",
            "probe_id": "2cd4df44-a9ce-4594-aa77-831245e05486",
            "attempt": 1,
            "outer_launch_id": launch_id,
            "authority_sha256": "sha256:"
            + hashlib.sha256(authority_path.read_bytes()).hexdigest(),
            "contract_sha256": authority["contract_sha256"],
            "result_tree_sha256": "sha256:" + ("6" * 64),
            "sandbox_requested": requested_backend,
            "sandbox_backend": backend,
            "contained": True,
            "gate_key_read_denied": True,
            "proof_write_denied": True,
            "control_write_denied": True,
            "operator_capture_read_denied": True,
            "operator_capture_write_denied": True,
            "signing_env_scrubbed": True,
            "probe_sha256": "sha256:05d6c7c8e44cbd769e76beb24e60d5865236bd434f2cc2b0950f5d94e047a5dd",
            "gate_evaluator_sha256": evaluator_identity,
            "signature": "a" * 64,
        }
        intervention_path = raw / "intervention.json"
        write_json(intervention_path, observation)
        view = job_view(
            backend=requested_backend,
            phase="terminal",
            outcome="verified",
            stop_reason="verified",
            attempt_count=1,
            last_sequence=4,
        )
        events = [
            event(1, "created", lease_epoch=0),
            event(
                2,
                "attempt_started",
                detail={"attempt": 1, "run_id": JOB_ID},
            ),
            event(
                3,
                "child_linked",
                detail={"attempt": 1, "run_id": JOB_ID, "launch_id": launch_id},
            ),
            event(4, "verified"),
        ]
        report = job_report(
            backend=backend,
            phase="terminal",
            outcome="verified",
            stop_reason="verified",
            attempt_count=1,
            lease_epoch=1,
            last_sequence=4,
        )
        report["receipt"]["receipt"] = {
            "job_id": JOB_ID,
            "run_id": JOB_ID,
            "authority_sha256": observation["authority_sha256"],
            "contract_sha256": observation["contract_sha256"],
            "result_tree_sha256": observation["result_tree_sha256"],
            "sandbox_backend": backend,
            "contained": True,
            "sandbox_boundary_observation_sha256": "sha256:"
            + hashlib.sha256(intervention_path.read_bytes()).hexdigest(),
        }
        fixtures = {
            "authority": ("json", authority),
            "job-view-after": ("json", view),
            "events-after": ("jsonl", events),
            "job-report": ("json", report),
        }
        captures = {}
        for name, (evidence_format, value) in fixtures.items():
            suffix = "jsonl" if evidence_format == "jsonl" else "json"
            path = raw / f"{name}.{suffix}"
            if evidence_format == "jsonl":
                write_jsonl(path, value)
            else:
                write_json(path, value)
            data = path.read_bytes()
            captures[name] = {
                "file": path.name,
                "format": evidence_format,
                "captured_at": "2026-07-30T12:01:00+00:00",
                "bytes": len(data),
                "sha256": "sha256:" + hashlib.sha256(data).hexdigest(),
                "provenance": "trusted_canonical",
                "source": {
                    "authority": "authority",
                    "job-view-after": "job-view",
                    "events-after": "job-events",
                    "job-report": "job-report",
                }[name],
                "phase": "before" if name == "authority" else "after",
            }
        intervention_bytes = intervention_path.read_bytes()
        state = {
            "capture_mode": "trusted",
            "trusted_capture": {"backend": backend},
            "intervention": {
                "status": "performed",
                "detail_sha256": "sha256:"
                + hashlib.sha256(intervention_bytes).hexdigest(),
            },
            "captures": captures,
        }
        declaration = {
            "id": "authoritative_attack_observation",
            "description": "The controller observation is bound.",
            "type": "sandbox_boundary_observation_bound",
            "backend": backend,
            "authority_evidence": "authority",
            "job_evidence": "job-view-after",
            "events_evidence": "events-after",
            "report_evidence": "job-report",
        }
        return state, declaration, observation, report

    def test_authenticated_sandbox_boundary_observation_binds_every_layer(self) -> None:
        state, declaration, _, _ = self.sandbox_boundary_oracle_fixture()
        assertion = RECORDER.evaluate_oracle(
            self.trial_dir,
            state,
            declaration,
            sandbox_boundary_probe_sha256="sha256:05d6c7c8e44cbd769e76beb24e60d5865236bd434f2cc2b0950f5d94e047a5dd",
        )
        self.assertEqual(assertion["status"], "passed", assertion)

    def test_auto_request_binds_to_the_concrete_cross_provider_backend(self) -> None:
        state, declaration, _, _ = self.sandbox_boundary_oracle_fixture(
            backend="sandbox-exec",
            requested_backend="auto",
        )
        declaration.pop("backend")
        assertion = RECORDER.evaluate_oracle(
            self.trial_dir,
            state,
            declaration,
            sandbox_boundary_probe_sha256="sha256:05d6c7c8e44cbd769e76beb24e60d5865236bd434f2cc2b0950f5d94e047a5dd",
        )
        self.assertEqual(assertion["status"], "passed", assertion)

    def test_sandbox_boundary_observation_mutations_fail_closed(self) -> None:
        mutations = {
            "extra field": lambda observation: observation.__setitem__("agent_claim", True),
            "missing denial": lambda observation: observation.pop("gate_key_read_denied"),
            "false denial": lambda observation: observation.__setitem__(
                "proof_write_denied", False
            ),
            "foreign job": lambda observation: observation.__setitem__(
                "job_id", "different-job"
            ),
            "foreign run": lambda observation: observation.__setitem__(
                "run_id", "different-run"
            ),
            "zero attempt": lambda observation: observation.__setitem__("attempt", 0),
            "foreign launch": lambda observation: observation.__setitem__(
                "outer_launch_id", "4f25f129-8d53-46db-b40b-6bd204272757"
            ),
            "backend substitution": lambda observation: observation.__setitem__(
                "sandbox_backend", "bwrap"
            ),
            "authority substitution": lambda observation: observation.__setitem__(
                "authority_sha256", "sha256:" + ("f" * 64)
            ),
            "evaluator substitution": lambda observation: observation.__setitem__(
                "gate_evaluator_sha256", "sha256:" + ("f" * 64)
            ),
            "probe substitution": lambda observation: observation.__setitem__(
                "probe_sha256", "sha256:" + ("f" * 64)
            ),
            "unsigned": lambda observation: observation.__setitem__("signature", ""),
        }
        for name, mutate in mutations.items():
            with self.subTest(name=name):
                state, declaration, observation, _ = self.sandbox_boundary_oracle_fixture()
                mutate(observation)
                intervention = self.trial_dir / "raw/intervention.json"
                write_json(intervention, observation)
                state["intervention"]["detail_sha256"] = "sha256:" + hashlib.sha256(
                    intervention.read_bytes()
                ).hexdigest()
                assertion = RECORDER.evaluate_oracle(
                    self.trial_dir,
                    state,
                    declaration,
                    sandbox_boundary_probe_sha256="sha256:05d6c7c8e44cbd769e76beb24e60d5865236bd434f2cc2b0950f5d94e047a5dd",
                )
                self.assertEqual(assertion["status"], "failed", assertion)

    def test_manual_or_receipt_unbound_boundary_observation_fails_closed(self) -> None:
        state, declaration, _, report = self.sandbox_boundary_oracle_fixture()
        state["capture_mode"] = "operator_attested"
        untrusted = RECORDER.evaluate_oracle(
            self.trial_dir,
            state,
            declaration,
            sandbox_boundary_probe_sha256="sha256:05d6c7c8e44cbd769e76beb24e60d5865236bd434f2cc2b0950f5d94e047a5dd",
        )
        self.assertEqual(untrusted["status"], "failed")

        state, declaration, _, report = self.sandbox_boundary_oracle_fixture()
        report["receipt"]["receipt"]["sandbox_boundary_observation_sha256"] = (
            "sha256:" + ("f" * 64)
        )
        report_path = self.trial_dir / "raw/job-report.json"
        write_json(report_path, report)
        report_bytes = report_path.read_bytes()
        state["captures"]["job-report"].update(
            {
                "bytes": len(report_bytes),
                "sha256": "sha256:" + hashlib.sha256(report_bytes).hexdigest(),
            }
        )
        unbound = RECORDER.evaluate_oracle(
            self.trial_dir,
            state,
            declaration,
            sandbox_boundary_probe_sha256="sha256:05d6c7c8e44cbd769e76beb24e60d5865236bd434f2cc2b0950f5d94e047a5dd",
        )
        self.assertEqual(unbound["status"], "failed")

    def test_synthetic_supervisor_flow_is_inconclusive_without_trusted_provenance(
        self,
    ) -> None:
        self.prepare()
        self.observe_supervisor()
        self.complete_cleanup()
        result = self.finalize()
        evaluation = result["evaluation"]
        self.assertEqual(evaluation["status"], "inconclusive")
        self.assertEqual(evaluation["backend"], "sandbox-exec")
        self.assertEqual(evaluation["job_id_prefix"], "01234567")
        self.assertEqual(
            result["capture_provenance"]["status"], "operator_attested"
        )
        self.assertIsNone(result["capture_provenance"]["receipt_sha256"])
        self.assertEqual(
            evaluation["intervention"]["kind"], "terminate_live_supervisor"
        )
        self.assertTrue(
            all(
                item["status"] == "passed"
                for item in evaluation["oracle_assertions"]
            )
        )
        evaluation_path = self.root / "sanitized-result.evaluation.json"
        self.assertEqual(
            result["evaluation_sha256"],
            "sha256:" + hashlib.sha256(evaluation_path.read_bytes()).hexdigest(),
        )
        state = json.loads(
            (self.trial_dir / "trial-state.json").read_text(encoding="utf-8")
        )
        self.assertLessEqual(
            state["captures"]["events-before"]["captured_at"],
            state["intervention"]["recorded_at"],
        )
        self.assertLessEqual(
            state["intervention"]["recorded_at"],
            state["captures"]["events-after"]["captured_at"],
        )
        RECORDER.validate_result_payload(result)

    def test_fake_helper_orchestrates_verified_two_file_flow_and_retries(self) -> None:
        (self.root / "fail-prepare-once").touch()
        with self.assertRaises(subprocess.CalledProcessError):
            self.prepare(trusted=True)
        first_state = json.loads(
            (self.trial_dir / "trial-state.json").read_text(encoding="utf-8")
        )
        self.assertEqual(
            first_state["capture_provenance"]["status"], "trusted_pending"
        )
        self.prepare(trusted=True)
        retried_state = json.loads(
            (self.trial_dir / "trial-state.json").read_text(encoding="utf-8")
        )
        self.assertEqual(first_state["session_id"], retried_state["session_id"])

        before_args, after_args = self.supervisor_captures()

        def canonical_args(captures: list[str]) -> list[str]:
            return [
                item
                for raw in captures[1::2]
                for item in ("--canonical", raw.split("=", 1)[0])
            ]

        run_recorder(
            "observe",
            "--trial-dir",
            str(self.trial_dir),
            *canonical_args(before_args),
        )
        run_recorder(
            "observe",
            "--trial-dir",
            str(self.trial_dir),
            "--intervention-status",
            "performed",
            "--intervention-detail-file",
            str(self.detail("trusted-intervention.txt")),
        )
        run_recorder(
            "observe",
            "--trial-dir",
            str(self.trial_dir),
            *canonical_args(after_args),
        )
        self.complete_cleanup()
        (self.root / "fail-seal-once").touch()
        (self.root / "fail-verify-once").touch()
        failed_after_evaluation = run_recorder(
            "finalize",
            "--trial-dir",
            str(self.trial_dir),
            "--output",
            str(self.output),
            check=False,
        )
        self.assertEqual(failed_after_evaluation.returncode, 2)
        evaluation_path = self.root / "sanitized-result.evaluation.json"
        first_evaluation_bytes = evaluation_path.read_bytes()
        failed_after_seal = run_recorder(
            "finalize",
            "--trial-dir",
            str(self.trial_dir),
            "--output",
            str(self.output),
            check=False,
        )
        self.assertEqual(failed_after_seal.returncode, 2)
        self.assertEqual(first_evaluation_bytes, evaluation_path.read_bytes())
        self.assertFalse(self.output.exists())
        (self.root / "fail-envelope-verify-once").touch()
        failed_after_envelope = run_recorder(
            "finalize",
            "--trial-dir",
            str(self.trial_dir),
            "--output",
            str(self.output),
            check=False,
        )
        self.assertEqual(failed_after_envelope.returncode, 2)
        envelope_bytes = self.output.read_bytes()
        result = self.finalize()
        self.assertEqual(envelope_bytes, self.output.read_bytes())
        first_bytes = self.output.read_bytes()
        result_retry = self.finalize()
        self.assertEqual(first_bytes, self.output.read_bytes())
        self.assertEqual(result, result_retry)

        evaluation = result["evaluation"]
        self.assertEqual(evaluation["status"], "passed")
        self.assertEqual(
            evaluation["capture_provenance"],
            {"status": "trusted_preseal", "receipt_sha256": None},
        )
        self.assertEqual(result["capture_provenance"]["status"], "verified")
        self.assertRegex(
            result["capture_provenance"]["publication_proof"],
            r"^[0-9a-f]{64}$",
        )
        self.assertEqual(
            result["evaluation_sha256"],
            "sha256:" + hashlib.sha256(evaluation_path.read_bytes()).hexdigest(),
        )
        replayed = RECORDER.build_evaluation(
            MANIFEST,
            self.trial_dir,
            json.loads(
                (self.trial_dir / "trial-state.json").read_text(encoding="utf-8")
            ),
            trusted_pass_ready=True,
            generated_at=evaluation["generated_at"],
        )
        self.assertEqual(
            RECORDER.pretty_json_bytes(replayed),
            evaluation_path.read_bytes(),
        )
        forged_assertion = copy.deepcopy(evaluation)
        forged_assertion["oracle_assertions"][0]["reason"] = "fabricated pass"
        self.assertNotEqual(
            RECORDER.pretty_json_bytes(forged_assertion),
            evaluation_path.read_bytes(),
        )
        log = [
            json.loads(line)
            for line in (self.root / "fake-capture-log.jsonl")
            .read_text(encoding="utf-8")
            .splitlines()
        ]
        commands = [entry[0] for entry in log]
        self.assertIn("prepare", commands)
        self.assertIn("inspect", commands)
        self.assertIn("seal", commands)
        self.assertGreaterEqual(commands.count("verify"), 2)
        observe_sources = {
            entry[entry.index("--source") + 1]
            for entry in log
            if entry[0] == "observe"
        }
        self.assertIn("job-intervention", observe_sources)
        self.assertIn("job-cleanup", observe_sources)
        result_text = self.output.read_text(encoding="utf-8")
        self.assertNotIn(str(self.fake_helper), result_text)
        self.assertNotIn(JOB_ID, result_text)

    def test_exclusive_result_create_refuses_precreated_symlink(self) -> None:
        target = self.root / "must-not-change.json"
        target.write_text("original\n", encoding="utf-8")
        link = self.root / "result-link.json"
        link.symlink_to(target)
        with self.assertRaisesRegex(RECORDER.TrialError, "already exists"):
            RECORDER.write_json_no_clobber(link, {"changed": True})
        self.assertEqual(target.read_text(encoding="utf-8"), "original\n")

    def test_atomic_publish_recovers_from_process_death_without_mutation(self) -> None:
        before_publish = self.root / "before-publish.json"
        after_publish = self.root / "after-publish.json"
        child = """
import importlib.util
import os
import sys

spec = importlib.util.spec_from_file_location("watchkeeper_live_trial", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
target = module.Path(sys.argv[2])
real_link = os.link
if sys.argv[3] == "before":
    def interrupted_link(*args, **kwargs):
        os._exit(91)
else:
    def interrupted_link(*args, **kwargs):
        real_link(*args, **kwargs)
        os._exit(92)
module.os.link = interrupted_link
module.write_json_no_clobber(target, {"durable": True})
"""
        interrupted_before = subprocess.run(
            [
                sys.executable,
                "-I",
                "-s",
                "-c",
                child,
                str(SCRIPT),
                str(before_publish),
                "before",
            ],
            check=False,
        )
        self.assertEqual(interrupted_before.returncode, 91)
        self.assertFalse(before_publish.exists())
        self.assertTrue(
            list(self.root.glob(".before-publish.json.deadreckon-capture-*.tmp"))
        )
        RECORDER.write_json_no_clobber(before_publish, {"durable": True})
        first_bytes = before_publish.read_bytes()
        RECORDER.write_json_no_clobber(before_publish, {"durable": True})
        self.assertEqual(before_publish.read_bytes(), first_bytes)

        interrupted_after = subprocess.run(
            [
                sys.executable,
                "-I",
                "-s",
                "-c",
                child,
                str(SCRIPT),
                str(after_publish),
                "after",
            ],
            check=False,
        )
        self.assertEqual(interrupted_after.returncode, 92)
        self.assertEqual(after_publish.read_bytes(), first_bytes)
        RECORDER.write_json_no_clobber(after_publish, {"durable": True})
        with self.assertRaisesRegex(RECORDER.TrialError, "different bytes"):
            RECORDER.write_json_no_clobber(after_publish, {"durable": False})
        self.assertEqual(after_publish.read_bytes(), first_bytes)

    def test_mutated_embedded_evaluation_cannot_reuse_bound_digests(self) -> None:
        self.prepare()
        self.observe_supervisor()
        self.complete_cleanup()
        result = self.finalize()
        forged = copy.deepcopy(result)
        forged["evaluation"]["limitations"].append("forged after publication")
        forged["capture_provenance"]["status"] = "verified"
        forged["capture_provenance"]["receipt_sha256"] = "sha256:" + ("a" * 64)
        forged["capture_provenance"]["publication_proof"] = "b" * 64
        with self.assertRaisesRegex(RECORDER.TrialError, "bound digest"):
            RECORDER.validate_result_payload(forged)

    def test_editing_mutable_state_cannot_forge_trusted_provenance(self) -> None:
        self.prepare()
        self.observe_supervisor()
        self.complete_cleanup()
        state_path = self.trial_dir / "trial-state.json"
        state = json.loads(state_path.read_text(encoding="utf-8"))
        state["capture_provenance"] = {
            "status": "verified",
            "receipt_sha256": "sha256:" + ("a" * 64),
            "reason": "forged by editing operator-owned state",
        }
        write_json(state_path, state)

        result = self.finalize()

        self.assertEqual(result["evaluation"]["status"], "inconclusive")
        self.assertEqual(
            result["capture_provenance"],
            {
                "status": "operator_attested",
                "receipt_sha256": None,
                "publication_proof": None,
            },
        )

    def test_schema_mismatch_missing_intervention_kind_is_rejected(self) -> None:
        self.prepare()
        self.observe_supervisor()
        self.complete_cleanup()
        result = self.finalize()
        malformed = copy.deepcopy(result)
        del malformed["evaluation"]["intervention"]["kind"]
        with self.assertRaisesRegex(
            RECORDER.TrialError, "malformed lifecycle|bound digest"
        ):
            RECORDER.validate_result_payload(malformed)

    def test_backend_allowlist_prevents_raw_path_or_content_leak(self) -> None:
        self.prepare()
        sensitive = "/secret/operator/path/RAW-CONTENT"
        self.observe_supervisor(backend=sensitive)
        self.complete_cleanup()
        state_path = self.trial_dir / "trial-state.json"
        state = json.loads(state_path.read_text(encoding="utf-8"))
        state["intervention"]["operator_note"] = sensitive
        state["cleanup"]["raw_path"] = sensitive
        write_json(state_path, state)
        result = self.finalize()
        result_text = self.output.read_text(encoding="utf-8")
        self.assertEqual(result["evaluation"]["status"], "inconclusive")
        self.assertEqual(result["evaluation"]["backend"], "unknown")
        self.assertNotIn(sensitive, result_text)
        self.assertNotIn(str(self.fixture_dir), result_text)
        self.assertNotIn(JOB_ID, result_text)

    def test_completed_cleanup_without_evidence_is_refused(self) -> None:
        self.prepare()
        self.observe_supervisor()
        completed = run_recorder(
            "cleanup",
            "--trial-dir",
            str(self.trial_dir),
            "--status",
            "completed",
            check=False,
        )
        self.assertEqual(completed.returncode, 2)
        self.assertIn("requires --detail-file", completed.stderr)
        result = self.finalize()
        self.assertEqual(result["evaluation"]["status"], "inconclusive")

    def test_performed_intervention_without_evidence_is_refused(self) -> None:
        self.prepare()
        completed = run_recorder(
            "observe",
            "--trial-dir",
            str(self.trial_dir),
            "--intervention-status",
            "performed",
            check=False,
        )
        self.assertEqual(completed.returncode, 2)
        self.assertIn("requires --intervention-detail-file", completed.stderr)

    def test_after_evidence_cannot_be_backfilled_before_intervention(self) -> None:
        self.prepare()
        _, after_captures = self.supervisor_captures()
        completed = run_recorder(
            "observe",
            "--trial-dir",
            str(self.trial_dir),
            *after_captures,
            check=False,
        )
        self.assertEqual(completed.returncode, 2)
        self.assertIn("current capture phase is before", completed.stderr)

    def test_stale_reclaim_and_adoption_events_cannot_pass(self) -> None:
        self.prepare()
        self.observe_supervisor(stale_after=True)
        self.complete_cleanup()
        result = self.finalize()
        evaluation = result["evaluation"]
        self.assertNotEqual(evaluation["status"], "passed")
        rejected = {
            item["id"]: item["status"] for item in evaluation["oracle_assertions"]
        }
        self.assertNotEqual(rejected["reclaim_bound_to_owner"], "passed")
        self.assertNotEqual(rejected["complete_child_identity_adopted"], "passed")

    def test_cross_job_report_cannot_pass(self) -> None:
        self.prepare()
        self.observe_supervisor(report_job_id="different-job")
        self.complete_cleanup()
        result = self.finalize()
        evaluation = result["evaluation"]
        self.assertNotEqual(evaluation["status"], "passed")
        rejected = {
            item["id"]: item["status"] for item in evaluation["oracle_assertions"]
        }
        self.assertNotEqual(rejected["report_bound_to_job"], "passed")
        self.assertNotEqual(rejected["recovery_within_policy"], "passed")

    def test_stale_report_counters_cannot_pass(self) -> None:
        self.prepare()
        self.observe_supervisor(report_last_sequence=999)
        self.complete_cleanup()
        result = self.finalize()
        evaluation = result["evaluation"]
        rejected = {
            item["id"]: item["status"] for item in evaluation["oracle_assertions"]
        }
        self.assertNotEqual(rejected["report_bound_to_job"], "passed")
        self.assertNotEqual(rejected["recovery_within_policy"], "passed")
        self.assertNotEqual(evaluation["status"], "passed")

    def test_nonverified_lifecycle_cannot_carry_a_valid_receipt(self) -> None:
        self.prepare()
        self.observe_supervisor(nonverified_receipt=True)
        self.complete_cleanup()
        result = self.finalize()
        evaluation = result["evaluation"]
        rejected = {
            item["id"]: item["status"] for item in evaluation["oracle_assertions"]
        }
        self.assertNotEqual(rejected["report_bound_to_job"], "passed")
        self.assertNotEqual(evaluation["status"], "passed")

    def test_definitive_failure_is_not_hidden_by_an_inconclusive_oracle(self) -> None:
        state = {
            "intervention": {
                "status": "performed",
                "detail_sha256": "sha256:" + ("a" * 64),
            },
            "cleanup": {
                "status": "completed",
                "detail_sha256": "sha256:" + ("b" * 64),
            },
            "captures": {"job-view-after": {}},
        }
        assertions = [
            {"status": "inconclusive"},
            {"status": "failed"},
        ]
        self.assertEqual(RECORDER.result_status(state, assertions), "failed")

    def test_fabricated_attack_and_campaign_summaries_are_refused(self) -> None:
        for trial_id, evidence_name in (
            ("cross_provider_gate_attack", "attack-observation"),
            ("live_campaign_interruption_recovery", "campaign-recovery-summary"),
        ):
            trial_dir = self.root / trial_id
            run_recorder(
                "prepare",
                trial_id,
                "--trial-dir",
                str(trial_dir),
                "--revision",
                REVISION,
            )
            fabricated = self.fixture_dir / f"{evidence_name}.json"
            write_json(
                fabricated,
                {
                    "gate_key_readable": False,
                    "forged_marker_accepted": False,
                    "duplicate_subplan_launches": 0,
                },
            )
            completed = run_recorder(
                "observe",
                "--trial-dir",
                str(trial_dir),
                "--capture",
                f"{evidence_name}={fabricated}",
                check=False,
            )
            self.assertEqual(completed.returncode, 2)
            self.assertIn("not declared evidence", completed.stderr)

    def test_symlinked_cleanup_evidence_is_refused(self) -> None:
        self.prepare()
        target = self.detail("real-cleanup.txt")
        link = self.fixture_dir / "cleanup-link.txt"
        link.symlink_to(target)
        completed = run_recorder(
            "cleanup",
            "--trial-dir",
            str(self.trial_dir),
            "--status",
            "completed",
            "--detail-file",
            str(link),
            check=False,
        )
        self.assertEqual(completed.returncode, 2)
        self.assertIn("non-symlink", completed.stderr)

    def test_observe_refuses_undeclared_or_symlinked_capture(self) -> None:
        self.prepare()
        unknown = self.fixture_dir / "unknown.json"
        write_json(unknown, {"safe": True})
        undeclared = run_recorder(
            "observe",
            "--trial-dir",
            str(self.trial_dir),
            "--capture",
            f"credentials={unknown}",
            check=False,
        )
        self.assertEqual(undeclared.returncode, 2)
        target = self.fixture_dir / "job-view-before.json"
        write_json(target, job_view())
        link = self.fixture_dir / "linked-job-view.json"
        link.symlink_to(target)
        symlinked = run_recorder(
            "observe",
            "--trial-dir",
            str(self.trial_dir),
            "--capture",
            f"job-view-before={link}",
            check=False,
        )
        self.assertEqual(symlinked.returncode, 2)
        self.assertIn("non-symlink", symlinked.stderr)

    def test_capture_mutation_after_observe_is_refused(self) -> None:
        self.prepare()
        before_captures, _ = self.supervisor_captures()
        run_recorder(
            "observe",
            "--trial-dir",
            str(self.trial_dir),
            *before_captures,
        )
        captured = self.trial_dir / "raw" / "job-view-before.json"
        value = json.loads(captured.read_text(encoding="utf-8"))
        value["job"]["projection"]["attempt_count"] = 999
        write_json(captured, value)

        finalized = run_recorder(
            "finalize",
            "--trial-dir",
            str(self.trial_dir),
            "--output",
            str(self.output),
            check=False,
        )
        self.assertEqual(finalized.returncode, 2)
        self.assertIn("changed after its observation boundary", finalized.stderr)

    def test_replace_is_refused_even_for_a_declared_capture(self) -> None:
        self.prepare()
        before_captures, _ = self.supervisor_captures()
        run_recorder(
            "observe",
            "--trial-dir",
            str(self.trial_dir),
            *before_captures,
        )
        replacement = run_recorder(
            "observe",
            "--trial-dir",
            str(self.trial_dir),
            "--replace",
            "--capture",
            before_captures[1],
            check=False,
        )
        self.assertEqual(replacement.returncode, 2)
        self.assertIn("append-only", replacement.stderr)

    def test_worker_stop_for_a_different_attempt_is_not_accepted(self) -> None:
        self.prepare("live_provider_worker_kill")
        child = supervised_child()
        before_events = [
            event(1, "created", lease_epoch=0),
            event(2, "lease_acquired", lease_epoch=1),
            event(3, "attempt_started", detail={"attempt": 1}),
            event(
                4,
                "child_linked",
                detail={
                    "pid": child["pid"],
                    "launch_id": child["launch_id"],
                    "attempt": child["attempt"],
                    "release_token_sha256": child["release_token_sha256"],
                    "boot_id": child["boot_id"],
                    "process_start_identity": child["process_start_identity"],
                },
            ),
        ]
        after_events = before_events + [
            event(
                5,
                "attempt_stopped",
                detail={"attempt": 99, "stop_reason": "transient_provider"},
            )
        ]
        fixtures = {
            "job-view-before": job_view(
                lease_epoch=1, last_sequence=len(before_events)
            ),
            "supervised-child-before": child,
        }
        before_captures: list[str] = []
        for name, value in fixtures.items():
            path = self.fixture_dir / f"{name}.json"
            write_json(path, value)
            before_captures.extend(["--capture", f"{name}={path}"])
        events_before = self.fixture_dir / "events-before.jsonl"
        write_jsonl(events_before, before_events)
        before_captures.extend(
            ["--capture", f"events-before={events_before}"]
        )
        run_recorder(
            "observe",
            "--trial-dir",
            str(self.trial_dir),
            *before_captures,
        )
        intervention = self.detail("worker-intervention.txt")
        run_recorder(
            "observe",
            "--trial-dir",
            str(self.trial_dir),
            "--intervention-status",
            "performed",
            "--intervention-detail-file",
            str(intervention),
        )
        after_values = {
            "job-view-after": job_view(
                lease_epoch=1, last_sequence=len(after_events)
            ),
            "job-report": job_report(
                lease_epoch=1, last_sequence=len(after_events)
            ),
        }
        after_captures: list[str] = []
        for name, value in after_values.items():
            path = self.fixture_dir / f"{name}.json"
            write_json(path, value)
            after_captures.extend(["--capture", f"{name}={path}"])
        events_after = self.fixture_dir / "events-after.jsonl"
        write_jsonl(events_after, after_events)
        after_captures.extend(["--capture", f"events-after={events_after}"])
        run_recorder(
            "observe",
            "--trial-dir",
            str(self.trial_dir),
            *after_captures,
        )
        self.complete_cleanup()
        result = self.finalize()
        assertions = {
            item["id"]: item["status"]
            for item in result["evaluation"]["oracle_assertions"]
        }
        self.assertNotEqual(assertions["worker_target_stopped"], "passed")

    def test_nested_plan_relaunch_event_fails_parent_only_oracle(self) -> None:
        raw = self.trial_dir / "raw"
        raw.mkdir(parents=True)
        artifact = {"plan_id": JOB_ID, "tasks": []}
        before = [
            {
                "timestamp": "2026-07-30T00:00:00Z",
                "plan_id": JOB_ID,
                "event": {"kind": "plan_started"},
            }
        ]
        after = before + [
            {
                "timestamp": "2026-07-30T00:00:01Z",
                "plan_id": JOB_ID,
                "event": {"kind": "task_started", "task_id": "task-1"},
            }
        ]
        state = {"captures": {}}
        for name, value, evidence_format in (
            ("parent-artifact-after", artifact, "json"),
            ("parent-events-before", before, "jsonl"),
            ("parent-events-after", after, "jsonl"),
        ):
            path = raw / f"{name}.{'json' if evidence_format == 'json' else 'jsonl'}"
            if evidence_format == "json":
                write_json(path, value)
            else:
                write_jsonl(path, value)
            data = path.read_bytes()
            state["captures"][name] = {
                "file": path.name,
                "format": evidence_format,
                "captured_at": "2026-07-30T00:00:00+00:00",
                "bytes": len(data),
                "sha256": "sha256:" + hashlib.sha256(data).hexdigest(),
                "provenance": "operator_supplied",
            }
        assertion = RECORDER.evaluate_oracle(
            self.trial_dir,
            state,
            {
                "id": "repair_is_parent_only",
                "description": "No child relaunch occurs.",
                "type": "parent_only_repair",
                "artifact_evidence": "parent-artifact-after",
                "before_evidence": "parent-events-before",
                "after_evidence": "parent-events-after",
            },
        )
        self.assertEqual(assertion["status"], "failed")


if __name__ == "__main__":
    unittest.main()
