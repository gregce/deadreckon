#!/usr/bin/env python3
"""Adversarial tests for Watchkeeper's verified completion metrics."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("collect-metrics.py")
HARNESS = Path(__file__).with_name("run.sh")


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


class VerifiedCompletionMetricsTests(unittest.TestCase):
    def collect(
        self, *, report_status: str, finish_succeeded: bool
    ) -> dict[str, object]:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            home = root / "home"
            observation = root / "observations" / "task-forged" / "job-forged"
            job_dir = home / "jobs" / "job-forged"

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
                    "job_id": "job-forged",
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
                        "    print(json.dumps({'job': {'projection': {'phase': 'terminal', 'outcome': 'verified', 'stop_reason': 'verified'}}}))",
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


if __name__ == "__main__":
    unittest.main()
