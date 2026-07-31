#!/usr/bin/env python3
"""Run Watchkeeper's credential-free adversarial proof matrix."""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class ProofCommand:
    argv: tuple[str, ...]
    expected_test: str


@dataclass(frozen=True)
class Trial:
    trial_id: str
    claim: str
    proof_type: str
    commands: tuple[ProofCommand, ...]
    limitation: str
    macos_sandbox_required: bool = False
    docker_required: bool = False


TRIALS = (
    Trial(
        "terminal_detach",
        "Closing the process which launched a Job does not stop its detached worker.",
        "hermetic_process",
        (
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--bin",
                    "deadreckon",
                    "--no-default-features",
                    "commands::supervisor::tests::closing_start_parent_does_not_stop_job",
                    "--",
                    "--exact",
                ),
                "commands::supervisor::tests::closing_start_parent_does_not_stop_job",
            ),
        ),
        "This kills a real launcher process and observes its child, but does not use a paid provider.",
    ),
    Trial(
        "worker_kill",
        "A killed worker is stopped as a process group and the Job schedules only a bounded exact-run resume.",
        "hermetic_process_and_reducer",
        (
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon-sandbox",
                    "--lib",
                    "tests::subprocess_cancel_escalates_sigterm_to_sigkill",
                    "--",
                    "--exact",
                ),
                "tests::subprocess_cancel_escalates_sigterm_to_sigkill",
            ),
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--bin",
                    "deadreckon",
                    "--no-default-features",
                    "commands::supervisor::tests::interrupted_leaf_with_a_dead_child_schedules_a_bounded_resume",
                    "--",
                    "--exact",
                ),
                "commands::supervisor::tests::interrupted_leaf_with_a_dead_child_schedules_a_bounded_resume",
            ),
        ),
        "The process kill is real. Recovery uses a persisted fixture rather than a live provider session.",
    ),
    Trial(
        "supervisor_restart",
        "A replacement supervisor preserves Job identity, workspace, budget and attempt state.",
        "hermetic_restart",
        (
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--bin",
                    "deadreckon",
                    "--no-default-features",
                    "commands::supervisor_service::tests::service_restart_resumes_same_job_workspace_budget_and_attempt",
                    "--",
                    "--exact",
                ),
                "commands::supervisor_service::tests::service_restart_resumes_same_job_workspace_budget_and_attempt",
            ),
        ),
        "This proves restart reconstruction from disk, not a live launchd, systemd or machine reboot.",
    ),
    Trial(
        "network_denial",
        "A contained worker cannot reach an outbound host when network access is denied.",
        "host_sandbox",
        (
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon-sandbox",
                    "--lib",
                    "tests::sandbox_blocks_outbound_to_evil_host",
                    "--",
                    "--exact",
                ),
                "tests::sandbox_blocks_outbound_to_evil_host",
            ),
        ),
        "This proves the local Seatbelt backend only; other hosts need their own live backend trial.",
        macos_sandbox_required=True,
    ),
    Trial(
        "gate_key_search_and_forgery",
        "A hostile contained worker cannot find the gate key or forge a marker.",
        "host_sandbox",
        (
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon-sandbox",
                    "--lib",
                    "tests::hostile_agent_cannot_find_keys_or_forge_marker_macos",
                    "--",
                    "--exact",
                ),
                "tests::hostile_agent_cannot_find_keys_or_forge_marker_macos",
            ),
        ),
        "This proves the local Seatbelt backend only; other hosts need their own live backend trial.",
        macos_sandbox_required=True,
    ),
    Trial(
        "docker_control_boundary",
        "A real Docker worker cannot read gate signing material, mutate Job, proof, gate or Git control paths, inherit signing inputs, or retain a network route.",
        "host_docker_sandbox",
        (
            ProofCommand(
                (
                    "env",
                    "DEADRECKON_LIVE_DOCKER_TEST=1",
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon-sandbox",
                    "--lib",
                    "tests::live_docker_denies_control_tampering_and_gate_inputs",
                    "--",
                    "--ignored",
                    "--exact",
                ),
                "tests::live_docker_denies_control_tampering_and_gate_inputs",
            ),
        ),
        "This executes the common boundary in a real Linux container. A macOS host cannot execute its Mach-O dr-gate inside that container, so the public strict Docker Job path remains a separate live claim.",
        docker_required=True,
    ),
    Trial(
        "receipt_mutation",
        "Changing a receipt signature blocks validation and promotion.",
        "hermetic_cryptographic_boundary",
        (
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon-core",
                    "--test",
                    "watchkeeper_fault_matrix",
                    "fault_matrix_covers_every_durable_boundary",
                    "--",
                    "--exact",
                ),
                "fault_matrix_covers_every_durable_boundary",
            ),
        ),
        "This mutates a real sealed receipt fixture; a separate operator trial still covers public finish.",
    ),
    Trial(
        "result_delivery",
        "Finish validates and delivers the receipt-bound parent result after crash recovery.",
        "hermetic_parent_delivery",
        (
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--bin",
                    "deadreckon",
                    "--no-default-features",
                    "commands::supervisor::tests::verified_graph_parent_is_promoted_and_finish_delivers_receipt_bound_output_after_crash_resume",
                    "--",
                    "--exact",
                ),
                "commands::supervisor::tests::verified_graph_parent_is_promoted_and_finish_delivers_receipt_bound_output_after_crash_resume",
            ),
        ),
        "This uses the same finish validator and materializer, but not a paid-provider public CLI run.",
    ),
    Trial(
        "unified_job_journey",
        "Ordinary run, orchestration, stored-plan fork, supported chain and campaign launches enter one durable Job lifecycle and five-command journey.",
        "hermetic_job_routing",
        (
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--bin",
                    "deadreckon",
                    "--no-default-features",
                    "commands::run::durable_direct_tests::direct_run_persists_one_bounded_job_with_the_same_run_identity",
                    "--",
                    "--exact",
                ),
                "commands::run::durable_direct_tests::direct_run_persists_one_bounded_job_with_the_same_run_identity",
            ),
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--bin",
                    "deadreckon",
                    "--no-default-features",
                    "commands::orchestrate::tests::direct_orchestration_persists_one_bounded_graph_job_with_parent_identity",
                    "--",
                    "--exact",
                ),
                "commands::orchestrate::tests::direct_orchestration_persists_one_bounded_graph_job_with_parent_identity",
            ),
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--bin",
                    "deadreckon",
                    "--no-default-features",
                    "commands::plan::tests::cli_fork_queues_a_job_while_trusted_drivers_use_the_inner_executor",
                    "--",
                    "--exact",
                ),
                "commands::plan::tests::cli_fork_queues_a_job_while_trusted_drivers_use_the_inner_executor",
            ),
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--bin",
                    "deadreckon",
                    "--no-default-features",
                    "commands::chain::tests::durable_chain_compiles_to_a_strict_linear_graph",
                    "--",
                    "--exact",
                ),
                "commands::chain::tests::durable_chain_compiles_to_a_strict_linear_graph",
            ),
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--bin",
                    "deadreckon",
                    "--no-default-features",
                    "commands::job::tests::campaign_start_also_enters_the_durable_job_queue",
                    "--",
                    "--exact",
                ),
                "commands::job::tests::campaign_start_also_enters_the_durable_job_queue",
            ),
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--bin",
                    "deadreckon",
                    "--no-default-features",
                    "commands::reference::tests::every_listed_job_has_a_non_looping_five_command_journey",
                    "--",
                    "--exact",
                ),
                "commands::reference::tests::every_listed_job_has_a_non_looping_five_command_journey",
            ),
        ),
        "This proves the supported creation routes and operator projection; historical compatibility modes remain deliberately outside the trusted Job path.",
    ),
    Trial(
        "job_child_and_planner_boundaries",
        "Job-owned child Runs cannot bypass their parent lifecycle, and hostile read-only planning cannot modify the operator workspace.",
        "hermetic_authority_and_sandbox",
        (
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--test",
                    "watchkeeper_job_ownership",
                    "public_resume_and_extend_cannot_mutate_a_job_owned_child",
                    "--",
                    "--exact",
                ),
                "public_resume_and_extend_cannot_mutate_a_job_owned_child",
            ),
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon-providers",
                    "--test",
                    "cli_providers",
                    "enforceably_read_only_request_denies_a_hostile_cli_workspace_write",
                    "--",
                    "--exact",
                ),
                "enforceably_read_only_request_denies_a_hostile_cli_workspace_write",
            ),
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon-providers",
                    "--test",
                    "cli_providers",
                    "enforceably_read_only_request_remains_usable_with_an_operational_sandbox",
                    "--",
                    "--exact",
                ),
                "enforceably_read_only_request_remains_usable_with_an_operational_sandbox",
            ),
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon-providers",
                    "--lib",
                    "types::tests::enforceably_read_only_request_carries_a_real_boundary",
                    "--",
                    "--exact",
                ),
                "types::tests::enforceably_read_only_request_carries_a_real_boundary",
            ),
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon-providers",
                    "--lib",
                    "types::tests::uncontained_worker_policy_cannot_weaken_a_read_only_request",
                    "--",
                    "--exact",
                ),
                "types::tests::uncontained_worker_policy_cannot_weaken_a_read_only_request",
            ),
        ),
        "The hostile planner proof exercises the selected local sandbox or a fail-closed host; live cross-provider and Linux/Docker execution remain separate operator trials.",
    ),
    Trial(
        "semantic_parent_repair",
        "Graph and Campaign semantic revise decisions run bounded parent-only repairs, recover candidate-ready work and preserve one parent Job identity.",
        "hermetic_parent_repair",
        (
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--bin",
                    "deadreckon",
                    "--no-default-features",
                    "commands::supervisor::tests::graph_parent_semantic_revise_uses_a_fenced_attempt_then_verifies_the_same_job",
                    "--",
                    "--exact",
                ),
                "commands::supervisor::tests::graph_parent_semantic_revise_uses_a_fenced_attempt_then_verifies_the_same_job",
            ),
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--bin",
                    "deadreckon",
                    "--no-default-features",
                    "commands::supervisor::tests::graph_parent_semantic_revise_twice_archives_each_round_then_verifies_the_same_job",
                    "--",
                    "--exact",
                ),
                "commands::supervisor::tests::graph_parent_semantic_revise_twice_archives_each_round_then_verifies_the_same_job",
            ),
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--bin",
                    "deadreckon",
                    "--no-default-features",
                    "commands::supervisor::tests::campaign_parent_semantic_revise_repairs_only_the_parent_then_verifies_the_same_job",
                    "--",
                    "--exact",
                ),
                "commands::supervisor::tests::campaign_parent_semantic_revise_repairs_only_the_parent_then_verifies_the_same_job",
            ),
        ),
        "This uses scripted providers and persisted crash fixtures; a naturally occurring live provider revise remains an operator trial.",
    ),
    Trial(
        "repair_lineage_tamper",
        "Receipt sealing and validation reject incomplete, unfenced, mutated or symlink-substituted parent-repair evidence.",
        "hermetic_repair_receipt_boundary",
        tuple(
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon-core",
                    "--lib",
                    f"completion::tests::{test_name}",
                    "--",
                    "--exact",
                ),
                f"completion::tests::{test_name}",
            )
            for test_name in (
                "repair_receipt_validates_full_fenced_parent_lineage",
                "repair_receipt_refuses_shape_mismatch_before_seal",
                "repair_receipt_refuses_unfenced_attempt_launch_and_lease_before_seal",
                "repair_receipt_refuses_candidate_result_tree_mismatch_before_seal",
                "repair_receipt_enforces_round_and_attempt_bounds",
                "repair_receipt_refuses_byte_identical_symlink_substitution",
                "repair_receipt_post_seal_mutation_matrix_fails_closed",
            )
        ),
        "This exercises real receipt and filesystem validation against fixtures; public finish tampering remains in the operator checklist.",
    ),
)


UNPROVEN = (
    {
        "id": "live_provider_worker_kill",
        "status": "unproven",
        "reason": "requires an approved live provider route and spend",
    },
    {
        "id": "live_provider_supervisor_restart",
        "status": "unproven",
        "reason": "requires an approved live provider route and host process intervention",
    },
    {
        "id": "live_provider_network_loss",
        "status": "unproven",
        "reason": "requires host network control during an approved live provider call",
    },
    {
        "id": "machine_reboot",
        "status": "unproven",
        "reason": "requires an installed active user service and a real reboot",
    },
    {
        "id": "cross_provider_gate_attack",
        "status": "unproven",
        "reason": "two provider routes were attempted, but neither attempt was a hostile cross-provider gate trial",
    },
    {
        "id": "live_provider_parent_repair",
        "status": "unproven",
        "reason": "requires a naturally occurring revise decision from an approved live semantic judge",
    },
    {
        "id": "live_campaign_interruption_recovery",
        "status": "unproven",
        "reason": "requires an approved live Campaign and host process intervention",
    },
    {
        "id": "linux_bubblewrap_gate_boundary",
        "status": "unproven",
        "reason": "requires a Linux host with an operational bubblewrap backend",
    },
    {
        "id": "docker_gate_boundary",
        "status": "unproven",
        "reason": "the real Docker control boundary is covered separately; this claim requires a public strict Job whose platform-compatible dr-gate runs inside the container",
    },
)


def digest(data: bytes) -> str:
    return f"sha256:{hashlib.sha256(data).hexdigest()}"


def run_command(repo: Path, proof: ProofCommand) -> dict[str, Any]:
    env = os.environ.copy()
    env["CARGO_TERM_COLOR"] = "never"
    env["RUST_TEST_THREADS"] = "1"
    started = time.monotonic()
    completed = subprocess.run(
        proof.argv,
        cwd=repo,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    elapsed = round(time.monotonic() - started, 3)
    stdout = completed.stdout.decode("utf-8", errors="replace")
    stderr = completed.stderr.decode("utf-8", errors="replace")
    matching = [
        line.strip()
        for line in (stdout + "\n" + stderr).splitlines()
        if proof.expected_test in line
    ]
    observed_pass = completed.returncode == 0 and any(
        "ok" in line or "test result: ok" in line for line in matching
    )
    return {
        "argv": list(proof.argv),
        "returncode": completed.returncode,
        "duration_seconds": elapsed,
        "stdout_sha256": digest(completed.stdout),
        "stderr_sha256": digest(completed.stderr),
        "expected_test": proof.expected_test,
        "matching_output": matching,
        "observed_pass": observed_pass,
    }


def seatbelt_preflight() -> dict[str, Any]:
    binary = shutil.which("sandbox-exec")
    if platform.system() != "Darwin" or binary is None:
        return {
            "available": binary is not None,
            "operational": False,
            "reason": "macOS sandbox-exec is unavailable",
        }
    completed = subprocess.run(
        (
            binary,
            "-p",
            "(version 1) (allow default)",
            "/usr/bin/true",
        ),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    return {
        "available": True,
        "operational": completed.returncode == 0,
        "returncode": completed.returncode,
        "stdout_sha256": digest(completed.stdout),
        "stderr_sha256": digest(completed.stderr),
        "reason": (
            None
            if completed.returncode == 0
            else "sandbox-exec could not apply a nested profile on this host"
        ),
    }


def docker_preflight() -> dict[str, Any]:
    binary = shutil.which("docker")
    if binary is None:
        return {
            "available": False,
            "operational": False,
            "image_cached": False,
            "reason": "docker is unavailable",
        }
    daemon = subprocess.run(
        (binary, "version", "--format", "{{.Server.Version}}"),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    image = subprocess.run(
        (binary, "image", "inspect", "rust:1", "--format", "{{.Id}}"),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    operational = daemon.returncode == 0 and image.returncode == 0
    return {
        "available": True,
        "operational": operational,
        "image_cached": image.returncode == 0,
        "daemon_returncode": daemon.returncode,
        "image_returncode": image.returncode,
        "daemon_stdout_sha256": digest(daemon.stdout),
        "daemon_stderr_sha256": digest(daemon.stderr),
        "image_stdout_sha256": digest(image.stdout),
        "image_stderr_sha256": digest(image.stderr),
        "reason": (
            None
            if operational
            else "docker daemon is unavailable or rust:1 is not already cached; the credential-free runner never pulls images implicitly"
        ),
    }


def run_trial(
    repo: Path,
    trial: Trial,
    seatbelt: dict[str, Any],
    docker: dict[str, Any],
) -> dict[str, Any]:
    if trial.macos_sandbox_required and not seatbelt["operational"]:
        return {
            "id": trial.trial_id,
            "claim": trial.claim,
            "proof_type": trial.proof_type,
            "status": "unproven",
            "reason": seatbelt["reason"],
            "limitation": trial.limitation,
            "commands": [],
        }
    if trial.docker_required and not docker["operational"]:
        return {
            "id": trial.trial_id,
            "claim": trial.claim,
            "proof_type": trial.proof_type,
            "status": "unproven",
            "reason": docker["reason"],
            "limitation": trial.limitation,
            "commands": [],
        }
    commands = [run_command(repo, command) for command in trial.commands]
    passed = all(command["observed_pass"] for command in commands)
    return {
        "id": trial.trial_id,
        "claim": trial.claim,
        "proof_type": trial.proof_type,
        "status": "passed" if passed else "failed",
        "reason": None if passed else "one or more authoritative commands failed",
        "limitation": trial.limitation,
        "commands": commands,
    }


def git_value(repo: Path, *args: str) -> str | None:
    result = subprocess.run(
        ("git", *args),
        cwd=repo,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=False,
        text=True,
    )
    value = result.stdout.strip()
    return value if result.returncode == 0 and value else None


def matrix_status(repo: Path) -> dict[str, Any]:
    matrix = json.loads(
        (repo / "examples/watchkeeper-dogfood/matrix.json").read_text(encoding="utf-8")
    )
    counts: dict[str, int] = {}
    for task in matrix["tasks"]:
        status = task["execution_status"]
        counts[status] = counts.get(status, 0) + 1
    return {
        "total_tasks": len(matrix["tasks"]),
        "by_execution_status": counts,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--repo",
        type=Path,
        default=Path(__file__).resolve().parents[2],
        help="DeadReckon repository root",
    )
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--only",
        action="append",
        choices=[trial.trial_id for trial in TRIALS],
        help="Run only one named credential-free trial; repeat for more",
    )
    args = parser.parse_args()
    repo = args.repo.resolve()
    selected = [
        trial for trial in TRIALS if args.only is None or trial.trial_id in args.only
    ]
    seatbelt = seatbelt_preflight()
    docker = docker_preflight()
    trials = [run_trial(repo, trial, seatbelt, docker) for trial in selected]
    payload = {
        "schema_version": 1,
        "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "credential_free": True,
        "repository": {
            "revision": git_value(repo, "rev-parse", "HEAD"),
            "dirty": bool(git_value(repo, "status", "--porcelain")),
        },
        "host": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
            "seatbelt_preflight": seatbelt,
            "docker_preflight": docker,
        },
        "runner_sha256": digest(Path(__file__).read_bytes()),
        "summary": {
            "passed": sum(trial["status"] == "passed" for trial in trials),
            "failed": sum(trial["status"] == "failed" for trial in trials),
            "unproven": sum(trial["status"] == "unproven" for trial in trials)
            + len(UNPROVEN),
        },
        "trials": trials,
        "live_claims": list(UNPROVEN),
        "matrix_status": matrix_status(repo),
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(args.output)
    return 1 if any(trial["status"] == "failed" for trial in trials) else 0


if __name__ == "__main__":
    sys.exit(main())
