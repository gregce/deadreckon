#!/usr/bin/env python3
"""Adversarial tests for Watchkeeper's verified completion metrics."""

from __future__ import annotations

import json
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("collect-metrics.py")
HARNESS = Path(__file__).with_name("run.sh")
BATCH = Path(__file__).with_name("batch.py")


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def matrix_payload(task_ids: list[str]) -> dict[str, object]:
    return {
        "repositories": [{"slot": "repo", "path_env": "WATCHKEEPER_TEST_REPO"}],
        "providers": [
            {"slot": "provider", "route_env": "WATCHKEEPER_TEST_PROVIDER"}
        ],
        "tasks": [
            {
                "id": task_id,
                "repository": "repo",
                "provider": "provider",
                "goal": f"complete {task_id}",
                "max_spend_usd": 1.0,
            }
            for task_id in task_ids
        ],
    }


def diverse_matrix_payload(task_ids: list[str]) -> dict[str, object]:
    return {
        "repositories": [
            {"slot": "repo-a", "path_env": "WATCHKEEPER_TEST_REPO_A"},
            {"slot": "repo-b", "path_env": "WATCHKEEPER_TEST_REPO_B"},
        ],
        "providers": [
            {"slot": "provider-a", "route_env": "WATCHKEEPER_TEST_PROVIDER_A"},
            {"slot": "provider-b", "route_env": "WATCHKEEPER_TEST_PROVIDER_B"},
        ],
        "tasks": [
            {
                "id": task_id,
                "repository": f"repo-{'a' if index % 2 == 0 else 'b'}",
                "provider": f"provider-{'a' if index % 2 == 0 else 'b'}",
                "goal": f"complete {task_id}",
                "max_spend_usd": 1.0,
            }
            for index, task_id in enumerate(task_ids)
        ],
    }


def file_digest(path: Path) -> str:
    return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()


def write_terminal_observation(
    artifacts: Path,
    matrix: Path,
    task_id: str,
    job_id: str,
    *,
    outcome: str = "failed",
    stop_reason: str = "fatal_provider",
    repository_slot: str = "repo",
    provider_slot: str = "provider",
) -> Path:
    observation = artifacts / task_id / job_id
    write_json(
        observation / "job-view.json",
        {
            "kind": "job_status",
            "job": {
                "job": {"job_id": job_id},
                "projection": {
                    "phase": "terminal",
                    "outcome": outcome,
                    "stop_reason": stop_reason,
                },
                "attempts": [],
            },
        },
    )
    write_json(
        observation / "operator-run.json",
        {
            "schema_version": 1,
            "task_id": task_id,
            "job_id": job_id,
            "matrix_sha256": file_digest(matrix),
            "repository_slot": repository_slot,
            "provider_slot": provider_slot,
            "terminal_outcome": outcome,
            "terminal_stop_reason": stop_reason,
            "public_commands": ["start", "status"],
            "report_attempted": False,
            "receipt_validation_attempted": False,
            "finish_attempted": False,
        },
    )
    return observation


def write_human_review(observation: Path, job_id: str) -> None:
    write_json(
        observation / "human-review.json",
        {
            "schema_version": 1,
            "job_id": job_id,
            "reviewed_at": "2026-07-31T12:00:00+00:00",
            "reviewer": "watchkeeper-operator",
            "false_acceptance": False,
            "false_rejection": False,
            "operator_interventions": 0,
            "time_to_understand_seconds": 1.0,
            "notes": None,
        },
    )


def write_empty_event_history(home: Path, job_id: str) -> None:
    path = home / "jobs" / job_id / "job-events.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("", encoding="utf-8")


class VerifiedCompletionMetricsTests(unittest.TestCase):
    def collect(
        self, *, report_status: str, finish_succeeded: bool
    ) -> dict[str, object]:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            home = root / "home"
            observation = root / "observations" / "task-forged" / "job-forged"
            job_dir = home / "jobs" / "job-forged"
            matrix = root / "matrix.json"
            write_json(matrix, matrix_payload(["task-forged"]))

            write_json(
                observation / "job-view.json",
                {
                    "kind": "job_status",
                    "job": {
                        "job": {"job_id": "job-forged"},
                        "projection": {
                            "phase": "terminal",
                            "outcome": "verified",
                            "stop_reason": "verified",
                        },
                        "attempts": [],
                    },
                },
            )
            write_json(
                job_dir / "receipt.json",
                {
                    "job_id": "job-forged",
                    "outcome": "verified",
                    "proof_kind": "two_key_completion",
                    "contained": True,
                    "sandbox_backend": "sandbox-exec",
                    "signature": "forged-self-claim",
                },
            )
            write_json(
                observation / "job-report.json",
                {
                    "id": "job-forged",
                    "phase": "terminal",
                    "outcome": "verified",
                    "stop_reason": "verified",
                    "receipt": {
                        "status": report_status,
                        "contained": True,
                        "sandbox_backend": "sandbox-exec",
                        "signature_validation_error": (
                            None if report_status == "valid" else "signature mismatch"
                        ),
                        "receipt": {
                            "job_id": "job-forged",
                            "proof_kind": "two_key_completion",
                        },
                    },
                },
            )
            finish_status = 0 if finish_succeeded else 1
            write_json(
                observation / "operator-run.json",
                {
                    "task_id": "task-forged",
                    "job_id": "job-forged",
                    "matrix_sha256": file_digest(matrix),
                    "repository_slot": "repo",
                    "provider_slot": "provider",
                    "terminal_outcome": "verified",
                    "terminal_stop_reason": "verified",
                    "public_commands": ["start", "status", "report", "finish"],
                    "report_attempted": True,
                    "report_exit_status": 0,
                    "report_succeeded": True,
                    "receipt_validation_attempted": True,
                    "receipt_validation_source": "deadreckon report --json",
                    "receipt_validation_exit_status": (
                        0 if report_status == "valid" else 1
                    ),
                    "receipt_validated": report_status == "valid",
                    "finish_attempted": True,
                    "finish_exit_status": finish_status,
                    "finish_succeeded": finish_succeeded,
                },
            )

            completed = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--home",
                    str(home),
                    "--observations",
                    str(root / "observations"),
                    "--matrix",
                    str(matrix),
                ],
                check=True,
                capture_output=True,
                text=True,
            )
            return json.loads(completed.stdout)

    def test_forged_raw_receipt_cannot_inflate_verified_jobs(self) -> None:
        metrics = self.collect(report_status="invalid", finish_succeeded=True)
        self.assertEqual(metrics["persisted_facts"]["verified_jobs"], 0)
        self.assertEqual(
            metrics["persisted_facts"]["unattended_verified_jobs"], 0
        )
        self.assertFalse(metrics["campaign_completion"]["claim_allowed"])

    def test_validation_without_successful_finish_is_not_verified(self) -> None:
        metrics = self.collect(report_status="valid", finish_succeeded=False)
        self.assertEqual(metrics["persisted_facts"]["verified_jobs"], 0)
        self.assertEqual(
            metrics["persisted_facts"]["unattended_verified_jobs"], 0
        )


class HarnessReceiptValidationTests(unittest.TestCase):
    def run_harness(self, report_status: str) -> tuple[subprocess.CompletedProcess[str], dict]:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            repository = root / "repo"
            home = root / "home"
            artifacts = root / "artifacts"
            fake_deadreckon = root / "deadreckon"
            finish_marker = root / "finish-called"
            matrix = root / "matrix.json"
            repository.mkdir()
            write_json(
                home / "jobs" / "job-forged" / "receipt.json",
                {
                    "job_id": "job-forged",
                    "outcome": "verified",
                    "proof_kind": "two_key_completion",
                    "contained": True,
                    "signature": "forged-self-claim",
                },
            )
            write_json(
                matrix,
                {
                    "repositories": [
                        {"slot": "repo", "path_env": "WATCHKEEPER_TEST_REPO"}
                    ],
                    "providers": [
                        {
                            "slot": "provider",
                            "route_env": "WATCHKEEPER_TEST_PROVIDER",
                        }
                    ],
                    "tasks": [
                        {
                            "id": "forged-task",
                            "repository": "repo",
                            "provider": "provider",
                            "goal": "reject a forged receipt",
                            "max_spend_usd": 1.0,
                        }
                    ],
                },
            )
            signature_error = (
                None if report_status == "valid" else "signature mismatch"
            )
            report = {
                "id": "job-forged",
                "phase": "terminal",
                "outcome": "verified",
                "stop_reason": "verified",
                "receipt": {
                    "status": report_status,
                    "contained": True,
                    "sandbox_backend": "sandbox-exec",
                    "signature_validation_error": signature_error,
                    "receipt": {
                        "job_id": "job-forged",
                        "proof_kind": "two_key_completion",
                        "contained": True,
                    },
                },
            }
            status = {
                "job": {
                    "job": {"job_id": "job-forged"},
                    "projection": {
                        "phase": "terminal",
                        "outcome": "verified",
                        "stop_reason": "verified",
                    },
                    "attempts": [],
                }
            }
            fake_deadreckon.write_text(
                "\n".join(
                    [
                        "#!/usr/bin/env python3",
                        "import json",
                        "import os",
                        "from pathlib import Path",
                        "import sys",
                        "command = sys.argv[1]",
                        "if command == 'start':",
                        "    print(json.dumps({'dispatched': {'ids': ['job-forged']}}))",
                        "elif command == 'status':",
                        f"    print(json.dumps({status!r}))",
                        "elif command == 'report':",
                        f"    print(json.dumps({report!r}))",
                        "elif command == 'finish':",
                        "    Path(os.environ['WATCHKEEPER_FINISH_MARKER']).write_text('called\\n')",
                        "else:",
                        "    raise SystemExit(98)",
                        "",
                    ]
                ),
                encoding="utf-8",
            )
            fake_deadreckon.chmod(0o755)
            environment = os.environ.copy()
            environment.update(
                {
                    "DEADRECKON_DOGFOOD_EXECUTE": "1",
                    "DEADRECKON_DOGFOOD_MATRIX": str(matrix),
                    "DEADRECKON_DOGFOOD_ARTIFACTS": str(artifacts),
                    "DEADRECKON_DOGFOOD_MAX_POLLS": "1",
                    "DEADRECKON_HOME": str(home),
                    "DEADRECKON_BIN": str(fake_deadreckon),
                    "WATCHKEEPER_TEST_REPO": str(repository),
                    "WATCHKEEPER_TEST_PROVIDER": "smoke",
                    "WATCHKEEPER_FINISH_MARKER": str(finish_marker),
                }
            )
            completed = subprocess.run(
                ["bash", str(HARNESS), "forged-task"],
                capture_output=True,
                text=True,
                env=environment,
            )
            record = json.loads(
                (
                    artifacts
                    / "forged-task"
                    / "job-forged"
                    / "operator-run.json"
                ).read_text(encoding="utf-8")
            )
            record["finish_marker_present"] = finish_marker.is_file()
            return completed, record

    def test_invalid_public_report_stops_before_finish(self) -> None:
        completed, record = self.run_harness("invalid")
        self.assertNotEqual(completed.returncode, 0)
        self.assertTrue(record["report_succeeded"])
        self.assertFalse(record["receipt_validated"])
        self.assertFalse(record["finish_attempted"])
        self.assertFalse(record["finish_marker_present"])

    def test_valid_public_report_and_finish_are_recorded(self) -> None:
        completed, record = self.run_harness("valid")
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertTrue(record["receipt_validated"])
        self.assertTrue(record["finish_succeeded"])
        self.assertTrue(record["finish_marker_present"])
        self.assertEqual(record["repository_slot"], "repo")
        self.assertEqual(record["provider_slot"], "provider")
        self.assertRegex(record["matrix_sha256"], r"^sha256:[0-9a-f]{64}$")


class MatrixCampaignTests(unittest.TestCase):
    def run_metrics(
        self, root: Path, matrix: Path, artifacts: Path
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--home",
                str(root / "home"),
                "--observations",
                str(artifacts),
                "--matrix",
                str(matrix),
            ],
            capture_output=True,
            text=True,
        )

    def test_metrics_bind_matrix_and_refuse_completion_below_twenty(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            artifacts = root / "artifacts"
            write_json(matrix, matrix_payload(["task-1", "task-2"]))
            write_json(
                artifacts / "live" / "trial-1" / "job-view-after.json",
                {"kind": "separate live-fault capture"},
            )
            observation = write_terminal_observation(
                artifacts, matrix, "task-1", "job-1"
            )
            (observation / "job-report.json").write_text("", encoding="utf-8")
            completed = self.run_metrics(root, matrix, artifacts)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            metrics = json.loads(completed.stdout)
            self.assertEqual(metrics["matrix"]["sha256"], file_digest(matrix))
            self.assertEqual(
                metrics["matrix"]["tasks"],
                [
                    {
                        "id": "task-1",
                        "repository_slot": "repo",
                        "provider_slot": "provider",
                    },
                    {
                        "id": "task-2",
                        "repository_slot": "repo",
                        "provider_slot": "provider",
                    },
                ],
            )
            campaign = metrics["campaign_completion"]
            self.assertEqual(
                campaign["counts"],
                {
                    "total": 2,
                    "missing": 1,
                    "attempted": 1,
                    "completed": 1,
                    "verified": 0,
                    "reviewed": 0,
                },
            )
            self.assertEqual(campaign["task_ids"]["missing"], ["task-2"])
            self.assertFalse(campaign["claim_allowed"])

    def test_twenty_distinct_completed_tasks_allow_campaign_claim(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            artifacts = root / "artifacts"
            task_ids = [f"task-{index:02d}" for index in range(20)]
            write_json(matrix, diverse_matrix_payload(task_ids))
            for index, task_id in enumerate(task_ids):
                suffix = "a" if index % 2 == 0 else "b"
                observation = write_terminal_observation(
                    artifacts,
                    matrix,
                    task_id,
                    f"job-{index:02d}",
                    repository_slot=f"repo-{suffix}",
                    provider_slot=f"provider-{suffix}",
                )
                write_human_review(observation, f"job-{index:02d}")
                write_empty_event_history(root / "home", f"job-{index:02d}")
            completed = self.run_metrics(root, matrix, artifacts)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            metrics = json.loads(completed.stdout)
            campaign = metrics["campaign_completion"]
            self.assertEqual(campaign["status"], "complete")
            self.assertEqual(campaign["assessment_status"], "ready")
            self.assertTrue(campaign["claim_allowed"])
            self.assertEqual(campaign["counts"]["completed"], 20)
            self.assertEqual(campaign["counts"]["reviewed"], 20)
            self.assertEqual(campaign["completed_repository_slot_count"], 2)
            self.assertEqual(campaign["completed_provider_slot_count"], 2)

            extra = artifacts / task_ids[0] / "job-extra"
            write_json(extra / "job-view.json", {"kind": "malformed extra"})
            ambiguous = self.run_metrics(root, matrix, artifacts)
            self.assertEqual(ambiguous.returncode, 0, ambiguous.stderr)
            ambiguous_metrics = json.loads(ambiguous.stdout)
            self.assertEqual(
                ambiguous_metrics["campaign_completion"]["status"], "incomplete"
            )
            self.assertFalse(
                ambiguous_metrics["campaign_completion"]["claim_allowed"]
            )
            self.assertTrue(
                ambiguous_metrics["data_quality"]["invalid_terminal_artifacts"]
            )

    def test_twenty_tasks_in_one_repo_and_provider_do_not_complete_campaign(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            artifacts = root / "artifacts"
            task_ids = [f"task-{index:02d}" for index in range(20)]
            write_json(matrix, matrix_payload(task_ids))
            for index, task_id in enumerate(task_ids):
                observation = write_terminal_observation(
                    artifacts, matrix, task_id, f"job-{index:02d}"
                )
                write_human_review(observation, f"job-{index:02d}")
                write_empty_event_history(root / "home", f"job-{index:02d}")

            completed = self.run_metrics(root, matrix, artifacts)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            campaign = json.loads(completed.stdout)["campaign_completion"]
            self.assertEqual(campaign["status"], "incomplete")
            self.assertFalse(campaign["claim_allowed"])
            self.assertEqual(campaign["completed_repository_slot_count"], 1)
            self.assertEqual(campaign["completed_provider_slot_count"], 1)

    def test_unfilled_human_review_template_does_not_allow_claim(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            artifacts = root / "artifacts"
            task_ids = [f"task-{index:02d}" for index in range(20)]
            write_json(matrix, diverse_matrix_payload(task_ids))
            for index, task_id in enumerate(task_ids):
                suffix = "a" if index % 2 == 0 else "b"
                job_id = f"job-{index:02d}"
                observation = write_terminal_observation(
                    artifacts,
                    matrix,
                    task_id,
                    job_id,
                    repository_slot=f"repo-{suffix}",
                    provider_slot=f"provider-{suffix}",
                )
                if index == 0:
                    write_json(
                        observation / "human-review.json",
                        {
                            "schema_version": 1,
                            "job_id": job_id,
                            "reviewed_at": None,
                            "reviewer": None,
                            "false_acceptance": None,
                            "false_rejection": None,
                            "operator_interventions": None,
                            "time_to_understand_seconds": None,
                            "notes": None,
                        },
                    )
                else:
                    write_human_review(observation, job_id)
                write_empty_event_history(root / "home", job_id)

            completed = self.run_metrics(root, matrix, artifacts)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            metrics = json.loads(completed.stdout)
            campaign = metrics["campaign_completion"]
            self.assertEqual(campaign["status"], "complete")
            self.assertEqual(campaign["assessment_status"], "incomplete")
            self.assertFalse(campaign["claim_allowed"])
            self.assertEqual(campaign["counts"]["reviewed"], 19)
            self.assertTrue(metrics["data_quality"]["invalid_human_reviews"])

    def test_missing_event_history_blocks_assessment_claim(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            artifacts = root / "artifacts"
            task_ids = [f"task-{index:02d}" for index in range(20)]
            write_json(matrix, diverse_matrix_payload(task_ids))
            for index, task_id in enumerate(task_ids):
                suffix = "a" if index % 2 == 0 else "b"
                job_id = f"job-{index:02d}"
                observation = write_terminal_observation(
                    artifacts,
                    matrix,
                    task_id,
                    job_id,
                    repository_slot=f"repo-{suffix}",
                    provider_slot=f"provider-{suffix}",
                )
                write_human_review(observation, job_id)
                if index != 0:
                    write_empty_event_history(root / "home", job_id)

            completed = self.run_metrics(root, matrix, artifacts)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            metrics = json.loads(completed.stdout)
            campaign = metrics["campaign_completion"]
            self.assertEqual(campaign["status"], "complete")
            self.assertEqual(campaign["assessment_status"], "incomplete")
            self.assertFalse(campaign["claim_allowed"])
            self.assertIn(
                str(root / "home" / "jobs" / "job-00" / "job-events.jsonl"),
                metrics["data_quality"]["missing_factual_artifacts"],
            )

    def test_unknown_and_duplicate_observation_task_ids_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            artifacts = root / "artifacts"
            write_json(matrix, matrix_payload(["known"]))
            write_terminal_observation(artifacts, matrix, "unknown", "job-unknown")
            unknown = self.run_metrics(root, matrix, artifacts)
            self.assertNotEqual(unknown.returncode, 0)
            self.assertIn("unknown observation task ID unknown", unknown.stderr)

            (artifacts / "unknown").rename(artifacts / "known")
            first = artifacts / "known" / "job-unknown" / "operator-run.json"
            record = json.loads(first.read_text(encoding="utf-8"))
            record["task_id"] = "known"
            write_json(first, record)
            write_terminal_observation(artifacts, matrix, "known", "job-second")
            duplicate = self.run_metrics(root, matrix, artifacts)
            self.assertNotEqual(duplicate.returncode, 0)
            self.assertIn("duplicate observation task ID known", duplicate.stderr)

    def test_symlinked_report_cannot_be_counted_as_verified(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            artifacts = root / "artifacts"
            write_json(matrix, matrix_payload(["task-1"]))
            observation = write_terminal_observation(
                artifacts,
                matrix,
                "task-1",
                "job-1",
                outcome="verified",
                stop_reason="verified",
            )
            operator_path = observation / "operator-run.json"
            operator = json.loads(operator_path.read_text(encoding="utf-8"))
            operator.update(
                {
                    "public_commands": ["start", "status", "report", "finish"],
                    "report_attempted": True,
                    "report_exit_status": 0,
                    "report_succeeded": True,
                    "receipt_validation_attempted": True,
                    "receipt_validation_source": "deadreckon report --json",
                    "receipt_validation_exit_status": 0,
                    "receipt_validated": True,
                    "finish_attempted": True,
                    "finish_exit_status": 0,
                    "finish_succeeded": True,
                }
            )
            write_json(operator_path, operator)
            external = root / "external-report.json"
            write_json(
                external,
                {
                    "id": "job-1",
                    "phase": "terminal",
                    "outcome": "verified",
                    "stop_reason": "verified",
                    "receipt": {
                        "status": "valid",
                        "contained": True,
                        "sandbox_backend": "sandbox-exec",
                        "signature_validation_error": None,
                        "receipt": {
                            "job_id": "job-1",
                            "proof_kind": "two_key_completion",
                        },
                    },
                },
            )
            (observation / "job-report.json").symlink_to(external)
            write_empty_event_history(root / "home", "job-1")

            completed = self.run_metrics(root, matrix, artifacts)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            metrics = json.loads(completed.stdout)
            self.assertEqual(metrics["persisted_facts"]["verified_jobs"], 0)
            self.assertTrue(
                metrics["data_quality"]["invalid_report_artifacts"]
            )
            self.assertFalse(metrics["campaign_completion"]["claim_allowed"])

    def test_external_semantic_path_cannot_inflate_judge_spend(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            artifacts = root / "artifacts"
            write_json(matrix, matrix_payload(["task-1"]))
            observation = write_terminal_observation(
                artifacts, matrix, "task-1", "job-1"
            )
            external_proofs = root / "external" / "proofs"
            write_json(external_proofs / "turn-acceptance.json", {})
            write_json(
                external_proofs / "semantic-judgment.json",
                {"job_id": "job-1", "run_id": "job-1", "spend_usd": 1234.5},
            )
            view_path = observation / "job-view.json"
            view = json.loads(view_path.read_text(encoding="utf-8"))
            view["job"]["attempts"] = [
                {
                    "id": {
                        "scope": "scope-1",
                        "run_id": "job-1",
                        "short": "job-1",
                    },
                    "proof": {
                        "marker_path": str(
                            external_proofs / "turn-acceptance.json"
                        )
                    },
                    "spend": {"total_usd": 0.0, "wall_seconds": 0.0},
                }
            ]
            write_json(view_path, view)
            write_empty_event_history(root / "home", "job-1")

            completed = self.run_metrics(root, matrix, artifacts)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            metrics = json.loads(completed.stdout)
            self.assertEqual(metrics["persisted_facts"]["judge_spend_usd"], 0.0)
            self.assertTrue(
                metrics["data_quality"]["invalid_semantic_artifacts"]
            )
            self.assertFalse(metrics["campaign_completion"]["claim_allowed"])


class BatchResumeTests(unittest.TestCase):
    def test_default_is_read_only_even_when_execute_environment_is_set(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            artifacts = root / "artifacts"
            marker = root / "runner-called"
            runner = root / "runner"
            write_json(matrix, matrix_payload(["task-1"]))
            runner.write_text(f"#!/bin/sh\ntouch {marker}\n", encoding="utf-8")
            runner.chmod(0o755)
            environment = os.environ.copy()
            environment["DEADRECKON_DOGFOOD_EXECUTE"] = "1"
            completed = subprocess.run(
                [
                    sys.executable,
                    str(BATCH),
                    "--matrix",
                    str(matrix),
                    "--artifacts",
                    str(artifacts),
                    "--runner",
                    str(runner),
                ],
                capture_output=True,
                text=True,
                env=environment,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            plan = json.loads(completed.stdout)
            self.assertEqual(plan["mode"], "plan")
            self.assertFalse(plan["provider_execution"])
            self.assertFalse(marker.exists())

    def test_execute_requires_gate_and_skips_only_valid_terminal_tasks(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            artifacts = root / "artifacts"
            log = root / "runner.log"
            runner = root / "runner"
            write_json(matrix, matrix_payload(["done", "pending"]))
            write_terminal_observation(artifacts, matrix, "done", "job-done")
            runner.write_text(
                "#!/bin/sh\nprintf '%s\\n' \"$1\" >> \"$WATCHKEEPER_BATCH_LOG\"\n",
                encoding="utf-8",
            )
            runner.chmod(0o755)
            command = [
                sys.executable,
                str(BATCH),
                "--matrix",
                str(matrix),
                "--artifacts",
                str(artifacts),
                "--runner",
                str(runner),
                "--execute",
            ]
            refused = subprocess.run(command, capture_output=True, text=True)
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn("DEADRECKON_DOGFOOD_EXECUTE=1", refused.stderr)
            self.assertFalse(log.exists())

            environment = os.environ.copy()
            environment.update(
                {
                    "DEADRECKON_DOGFOOD_EXECUTE": "1",
                    "WATCHKEEPER_BATCH_LOG": str(log),
                }
            )
            executed = subprocess.run(
                command, capture_output=True, text=True, env=environment
            )
            self.assertEqual(executed.returncode, 0, executed.stderr)
            self.assertEqual(log.read_text(encoding="utf-8").splitlines(), ["pending"])

    def test_execute_propagates_the_reviewed_matrix_artifacts_and_digest(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            reviewed_matrix = root / "reviewed-matrix.json"
            inherited_matrix = root / "inherited-matrix.json"
            reviewed_artifacts = root / "reviewed-artifacts"
            inherited_artifacts = root / "inherited-artifacts"
            record_path = root / "runner-environment.json"
            runner = root / "runner"
            write_json(reviewed_matrix, matrix_payload(["reviewed-task"]))
            write_json(inherited_matrix, matrix_payload(["inherited-task"]))
            runner.write_text(
                "\n".join(
                    [
                        "#!/usr/bin/env python3",
                        "import json",
                        "import os",
                        "from pathlib import Path",
                        "record = {",
                        "    'task_id': __import__('sys').argv[1],",
                        "    'matrix': os.environ['DEADRECKON_DOGFOOD_MATRIX'],",
                        "    'artifacts': os.environ['DEADRECKON_DOGFOOD_ARTIFACTS'],",
                        "    'sha256': os.environ['DEADRECKON_DOGFOOD_MATRIX_SHA256'],",
                        "}",
                        "Path(os.environ['WATCHKEEPER_BATCH_RECORD']).write_text(",
                        "    json.dumps(record) + '\\n'",
                        ")",
                        "",
                    ]
                ),
                encoding="utf-8",
            )
            runner.chmod(0o755)
            environment = os.environ.copy()
            environment.update(
                {
                    "DEADRECKON_DOGFOOD_EXECUTE": "1",
                    "DEADRECKON_DOGFOOD_MATRIX": str(inherited_matrix),
                    "DEADRECKON_DOGFOOD_ARTIFACTS": str(inherited_artifacts),
                    "WATCHKEEPER_BATCH_RECORD": str(record_path),
                }
            )
            completed = subprocess.run(
                [
                    sys.executable,
                    str(BATCH),
                    "--matrix",
                    str(reviewed_matrix),
                    "--artifacts",
                    str(reviewed_artifacts),
                    "--runner",
                    str(runner),
                    "--execute",
                ],
                capture_output=True,
                text=True,
                env=environment,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            record = json.loads(record_path.read_text(encoding="utf-8"))
            self.assertEqual(record["task_id"], "reviewed-task")
            self.assertEqual(record["matrix"], str(reviewed_matrix))
            self.assertEqual(record["artifacts"], str(reviewed_artifacts))
            self.assertEqual(record["sha256"], file_digest(reviewed_matrix))

    def test_killed_runner_partial_artifacts_block_a_duplicate_restart(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            artifacts = root / "artifacts"
            log = root / "runner.log"
            runner = root / "runner"
            write_json(matrix, matrix_payload(["task-1"]))
            runner.write_text(
                "\n".join(
                    [
                        "#!/usr/bin/env python3",
                        "import json",
                        "import os",
                        "from pathlib import Path",
                        "import sys",
                        "task_id = sys.argv[1]",
                        "root = Path(os.environ['DEADRECKON_DOGFOOD_ARTIFACTS'])",
                        "observation = root / task_id / 'job-partial'",
                        "observation.mkdir(parents=True, exist_ok=True)",
                        "start = {'dispatched': {'ids': ['job-partial']}}",
                        "(observation / 'start.json').write_text(json.dumps(start) + '\\n')",
                        "status = {'job': {",
                        "    'job': {'job_id': 'job-partial'},",
                        "    'projection': {'phase': 'running'},",
                        "}}",
                        "(observation / 'status-latest.json').write_text(",
                        "    json.dumps(status) + '\\n'",
                        ")",
                        "with open(os.environ['WATCHKEEPER_BATCH_LOG'], 'a') as handle:",
                        "    handle.write(task_id + '\\n')",
                        "raise SystemExit(143)",
                        "",
                    ]
                ),
                encoding="utf-8",
            )
            runner.chmod(0o755)
            environment = os.environ.copy()
            environment.update(
                {
                    "DEADRECKON_DOGFOOD_EXECUTE": "1",
                    "DEADRECKON_DOGFOOD_ARTIFACTS": str(artifacts),
                    "WATCHKEEPER_BATCH_LOG": str(log),
                }
            )
            command = [
                sys.executable,
                str(BATCH),
                "--matrix",
                str(matrix),
                "--artifacts",
                str(artifacts),
                "--runner",
                str(runner),
            ]
            first = subprocess.run(
                [*command, "--execute"],
                capture_output=True,
                text=True,
                env=environment,
            )
            self.assertEqual(first.returncode, 143)
            self.assertEqual(log.read_text(encoding="utf-8").splitlines(), ["task-1"])

            plan = subprocess.run(command, capture_output=True, text=True, env=environment)
            self.assertEqual(plan.returncode, 0, plan.stderr)
            self.assertEqual(
                json.loads(plan.stdout)["tasks"][0]["action"], "blocked_partial"
            )
            restarted = subprocess.run(
                [*command, "--execute"],
                capture_output=True,
                text=True,
                env=environment,
            )
            self.assertEqual(restarted.returncode, 65)
            self.assertIn("incomplete", restarted.stderr)
            self.assertEqual(log.read_text(encoding="utf-8").splitlines(), ["task-1"])

    def test_invalid_terminal_operator_record_blocks_duplicate_execution(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            artifacts = root / "artifacts"
            marker = root / "runner-called"
            runner = root / "runner"
            write_json(matrix, matrix_payload(["task-1"]))
            observation = write_terminal_observation(
                artifacts, matrix, "task-1", "job-1"
            )
            record_path = observation / "operator-run.json"
            record = json.loads(record_path.read_text(encoding="utf-8"))
            record["matrix_sha256"] = "sha256:" + "0" * 64
            write_json(record_path, record)
            runner.write_text(f"#!/bin/sh\ntouch '{marker}'\n", encoding="utf-8")
            runner.chmod(0o755)
            environment = os.environ.copy()
            environment["DEADRECKON_DOGFOOD_EXECUTE"] = "1"
            completed = subprocess.run(
                [
                    sys.executable,
                    str(BATCH),
                    "--matrix",
                    str(matrix),
                    "--artifacts",
                    str(artifacts),
                    "--runner",
                    str(runner),
                    "--execute",
                ],
                capture_output=True,
                text=True,
                env=environment,
            )
            self.assertEqual(completed.returncode, 65)
            self.assertIn("blocked_invalid", completed.stdout)
            self.assertIn("archive or repair", completed.stderr)
            self.assertFalse(marker.exists())


class MatrixValidationTests(unittest.TestCase):
    def run_plan(self, root: Path, payload: dict[str, object]) -> subprocess.CompletedProcess[str]:
        matrix = root / "matrix.json"
        write_json(matrix, payload)
        return subprocess.run(
            [sys.executable, str(BATCH), "--matrix", str(matrix)],
            capture_output=True,
            text=True,
        )

    def test_shared_parser_rejects_unsafe_ids_and_unbounded_spend(self) -> None:
        cases: list[tuple[str, dict[str, object], str]] = []
        unsafe_task = matrix_payload(["task-1"])
        unsafe_task["tasks"][0]["id"] = "../escape"
        cases.append(("unsafe task", unsafe_task, "tasks[0].id"))

        unsafe_slot = matrix_payload(["task-1"])
        unsafe_slot["repositories"][0]["slot"] = "../repo"
        unsafe_slot["tasks"][0]["repository"] = "../repo"
        cases.append(("unsafe slot", unsafe_slot, "repositories[0].slot"))

        infinite = matrix_payload(["task-1"])
        infinite["tasks"][0]["max_spend_usd"] = float("inf")
        cases.append(("infinite spend", infinite, "finite and nonnegative"))

        negative = matrix_payload(["task-1"])
        negative["tasks"][0]["max_spend_usd"] = -1
        cases.append(("negative spend", negative, "finite and nonnegative"))

        for name, payload, expected in cases:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as raw_root:
                completed = self.run_plan(Path(raw_root), payload)
                self.assertNotEqual(completed.returncode, 0)
                self.assertIn(expected, completed.stderr)

    def test_direct_runner_uses_shared_task_count_validation_before_tools(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            marker = root / "deadreckon-called"
            fake_deadreckon = root / "deadreckon"
            payload = matrix_payload(["task-1"])
            payload["task_count"] = 2
            write_json(matrix, payload)
            fake_deadreckon.write_text(
                f"#!/bin/sh\ntouch '{marker}'\nexit 99\n", encoding="utf-8"
            )
            fake_deadreckon.chmod(0o755)
            environment = os.environ.copy()
            environment.update(
                {
                    "DEADRECKON_DOGFOOD_EXECUTE": "1",
                    "DEADRECKON_DOGFOOD_MATRIX": str(matrix),
                    "DEADRECKON_HOME": str(root / "home"),
                    "DEADRECKON_BIN": str(fake_deadreckon),
                }
            )
            completed = subprocess.run(
                ["bash", str(HARNESS), "task-1"],
                capture_output=True,
                text=True,
                env=environment,
            )
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("matrix task_count is 2", completed.stderr)
            self.assertFalse(marker.exists())

    def test_direct_runner_rejects_matrix_changed_after_batch_planning(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            repository = root / "repo"
            artifacts = root / "artifacts"
            marker = root / "deadreckon-called"
            fake_deadreckon = root / "deadreckon"
            repository.mkdir()
            write_json(matrix, matrix_payload(["task-1"]))
            reviewed_sha256 = file_digest(matrix)
            changed = matrix_payload(["task-1"])
            changed["tasks"][0]["goal"] = "unreviewed changed goal"
            write_json(matrix, changed)
            fake_deadreckon.write_text(
                f"#!/bin/sh\ntouch '{marker}'\nexit 99\n", encoding="utf-8"
            )
            fake_deadreckon.chmod(0o755)
            environment = os.environ.copy()
            environment.update(
                {
                    "DEADRECKON_DOGFOOD_EXECUTE": "1",
                    "DEADRECKON_DOGFOOD_MATRIX": str(matrix),
                    "DEADRECKON_DOGFOOD_MATRIX_SHA256": reviewed_sha256,
                    "DEADRECKON_DOGFOOD_ARTIFACTS": str(artifacts),
                    "DEADRECKON_HOME": str(root / "home"),
                    "DEADRECKON_BIN": str(fake_deadreckon),
                    "WATCHKEEPER_TEST_REPO": str(repository),
                    "WATCHKEEPER_TEST_PROVIDER": "smoke",
                }
            )
            completed = subprocess.run(
                ["bash", str(HARNESS), "task-1"],
                capture_output=True,
                text=True,
                env=environment,
            )
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("matrix bytes changed after batch planning", completed.stderr)
            self.assertFalse(marker.exists())
            self.assertFalse((artifacts / "task-1").exists())

    def test_direct_runner_rejects_a_traversing_dispatched_job_id(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            repository = root / "repo"
            artifacts = root / "artifacts"
            status_marker = root / "status-called"
            fake_deadreckon = root / "deadreckon"
            repository.mkdir()
            write_json(matrix, matrix_payload(["task-1"]))
            fake_deadreckon.write_text(
                "\n".join(
                    [
                        "#!/usr/bin/env python3",
                        "import json",
                        "import os",
                        "from pathlib import Path",
                        "import sys",
                        "if sys.argv[1] == 'start':",
                        "    print(json.dumps({'dispatched': {'ids': ['../../escaped-job']}}))",
                        "else:",
                        "    Path(os.environ['WATCHKEEPER_STATUS_MARKER']).write_text('called\\n')",
                        "    raise SystemExit(99)",
                        "",
                    ]
                ),
                encoding="utf-8",
            )
            fake_deadreckon.chmod(0o755)
            environment = os.environ.copy()
            environment.update(
                {
                    "DEADRECKON_DOGFOOD_EXECUTE": "1",
                    "DEADRECKON_DOGFOOD_MATRIX": str(matrix),
                    "DEADRECKON_DOGFOOD_ARTIFACTS": str(artifacts),
                    "DEADRECKON_HOME": str(root / "home"),
                    "DEADRECKON_BIN": str(fake_deadreckon),
                    "WATCHKEEPER_TEST_REPO": str(repository),
                    "WATCHKEEPER_TEST_PROVIDER": "smoke",
                    "WATCHKEEPER_STATUS_MARKER": str(status_marker),
                }
            )
            completed = subprocess.run(
                ["bash", str(HARNESS), "task-1"],
                capture_output=True,
                text=True,
                env=environment,
            )
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("public start job id", completed.stderr)
            self.assertFalse(status_marker.exists())
            self.assertFalse((root / "escaped-job" / "start.json").exists())

    def test_direct_runner_atomically_refuses_an_existing_empty_task_root(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            repository = root / "repo"
            artifacts = root / "artifacts"
            marker = root / "deadreckon-called"
            fake_deadreckon = root / "deadreckon"
            repository.mkdir()
            (artifacts / "task-1").mkdir(parents=True)
            write_json(matrix, matrix_payload(["task-1"]))
            fake_deadreckon.write_text(
                f"#!/bin/sh\ntouch '{marker}'\nexit 99\n", encoding="utf-8"
            )
            fake_deadreckon.chmod(0o755)
            environment = os.environ.copy()
            environment.update(
                {
                    "DEADRECKON_DOGFOOD_EXECUTE": "1",
                    "DEADRECKON_DOGFOOD_MATRIX": str(matrix),
                    "DEADRECKON_DOGFOOD_ARTIFACTS": str(artifacts),
                    "DEADRECKON_HOME": str(root / "home"),
                    "DEADRECKON_BIN": str(fake_deadreckon),
                    "WATCHKEEPER_TEST_REPO": str(repository),
                    "WATCHKEEPER_TEST_PROVIDER": "smoke",
                }
            )
            completed = subprocess.run(
                ["bash", str(HARNESS), "task-1"],
                capture_output=True,
                text=True,
                env=environment,
            )
            self.assertEqual(completed.returncode, 65)
            self.assertIn("already exists", completed.stderr)
            self.assertFalse(marker.exists())

    def test_second_direct_start_cannot_dispatch_a_second_job(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            repository = root / "repo"
            artifacts = root / "artifacts"
            log = root / "deadreckon.log"
            fake_deadreckon = root / "deadreckon"
            repository.mkdir()
            write_json(matrix, matrix_payload(["task-1"]))
            fake_deadreckon.write_text(
                "#!/bin/sh\nprintf '%s\\n' \"$1\" >> "
                f"'{log}'\nexit 99\n",
                encoding="utf-8",
            )
            fake_deadreckon.chmod(0o755)
            environment = os.environ.copy()
            environment.update(
                {
                    "DEADRECKON_DOGFOOD_EXECUTE": "1",
                    "DEADRECKON_DOGFOOD_MATRIX": str(matrix),
                    "DEADRECKON_DOGFOOD_ARTIFACTS": str(artifacts),
                    "DEADRECKON_HOME": str(root / "home"),
                    "DEADRECKON_BIN": str(fake_deadreckon),
                    "WATCHKEEPER_TEST_REPO": str(repository),
                    "WATCHKEEPER_TEST_PROVIDER": "smoke",
                }
            )
            command = ["bash", str(HARNESS), "task-1"]
            first = subprocess.run(
                command, capture_output=True, text=True, env=environment
            )
            self.assertEqual(first.returncode, 99)
            second = subprocess.run(
                command, capture_output=True, text=True, env=environment
            )
            self.assertNotEqual(second.returncode, 0)
            self.assertIn("duplicate Job", second.stderr)
            self.assertEqual(log.read_text(encoding="utf-8").splitlines(), ["start"])


class SymlinkArtifactTests(unittest.TestCase):
    def run_direct(
        self, root: Path, matrix: Path, artifacts: Path, marker: Path
    ) -> subprocess.CompletedProcess[str]:
        repository = root / "repo"
        fake_deadreckon = root / "deadreckon"
        repository.mkdir(exist_ok=True)
        fake_deadreckon.write_text(
            f"#!/bin/sh\ntouch '{marker}'\nexit 99\n", encoding="utf-8"
        )
        fake_deadreckon.chmod(0o755)
        environment = os.environ.copy()
        environment.update(
            {
                "DEADRECKON_DOGFOOD_EXECUTE": "1",
                "DEADRECKON_DOGFOOD_MATRIX": str(matrix),
                "DEADRECKON_DOGFOOD_ARTIFACTS": str(artifacts),
                "DEADRECKON_HOME": str(root / "home"),
                "DEADRECKON_BIN": str(fake_deadreckon),
                "WATCHKEEPER_TEST_REPO": str(repository),
                "WATCHKEEPER_TEST_PROVIDER": "smoke",
            }
        )
        return subprocess.run(
            ["bash", str(HARNESS), "task-1"],
            capture_output=True,
            text=True,
            env=environment,
        )

    def assert_all_entrypoints_reject(
        self, root: Path, matrix: Path, artifacts: Path, expected: str
    ) -> None:
        batch = subprocess.run(
            [
                sys.executable,
                str(BATCH),
                "--matrix",
                str(matrix),
                "--artifacts",
                str(artifacts),
            ],
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(batch.returncode, 0)
        self.assertIn(expected, batch.stderr)

        metrics = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--home",
                str(root / "home"),
                "--observations",
                str(artifacts),
                "--matrix",
                str(matrix),
            ],
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(metrics.returncode, 0)
        self.assertIn(expected, metrics.stderr)

        marker = root / "deadreckon-called"
        direct = self.run_direct(root, matrix, artifacts, marker)
        self.assertNotEqual(direct.returncode, 0)
        self.assertIn(expected, direct.stderr)
        self.assertFalse(marker.exists())

    def test_symlinked_task_root_is_rejected_everywhere(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            artifacts = root / "artifacts"
            target = root / "outside-task"
            target.mkdir()
            artifacts.mkdir()
            (artifacts / "task-1").symlink_to(target, target_is_directory=True)
            write_json(matrix, matrix_payload(["task-1"]))
            self.assert_all_entrypoints_reject(
                root, matrix, artifacts, "artifact root must not be a symlink"
            )

    def test_symlinked_job_candidate_is_rejected_everywhere(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            matrix = root / "matrix.json"
            artifacts = root / "artifacts"
            target = root / "outside-job"
            target.mkdir()
            (artifacts / "task-1").mkdir(parents=True)
            (artifacts / "task-1" / "job-1").symlink_to(
                target, target_is_directory=True
            )
            write_json(matrix, matrix_payload(["task-1"]))
            self.assert_all_entrypoints_reject(
                root, matrix, artifacts, "artifact candidate must not be a symlink"
            )


if __name__ == "__main__":
    unittest.main()
