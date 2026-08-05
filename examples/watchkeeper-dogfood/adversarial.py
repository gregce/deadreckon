#!/usr/bin/env python3
"""Run Watchkeeper's credential-free adversarial proof matrix."""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import platform
import re
import shutil
import stat
import struct
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any


BUNDLE_BUILD_ID_PATTERN = re.compile(
    rb"deadreckon-bundle-build-id-sha256:[a-f0-9]{64}"
)


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
    public_docker_required: bool = False


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
        "macos_developer_tool_gate",
        "A strict macOS gate resolves Apple developer-tool shims before containment and permits shell redirection only to the null device.",
        "host_sandbox_toolchain",
        (
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon-sandbox",
                    "--lib",
                    "commands::tests::disposable_seatbelt_allows_only_the_null_device_as_a_system_write",
                    "--",
                    "--exact",
                ),
                "commands::tests::disposable_seatbelt_allows_only_the_null_device_as_a_system_write",
            ),
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon-runtime",
                    "--lib",
                    "turn_loop::tests::strict_macos_gate_runs_controller_resolved_python_and_dev_null",
                    "--",
                    "--exact",
                ),
                "turn_loop::tests::strict_macos_gate_runs_controller_resolved_python_and_dev_null",
            ),
        ),
        "This executes real Seatbelt with the host Xcode toolchain, but does not consume a provider subscription or prove a complete public Job.",
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
        "This executes the common boundary in a real Linux container. Public strict Docker Job completion, cancellation and crash recovery remain a separate proof group.",
        docker_required=True,
    ),
    Trial(
        "docker_gate_boundary",
        "Public strict Docker Jobs complete deterministic verification, clean up after operator cancellation, and reconcile a killed worker before exactly one bounded retry.",
        "public_strict_docker_job",
        tuple(
            ProofCommand(
                (
                    "env",
                    "DEADRECKON_LIVE_DOCKER_TEST=1",
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--test",
                    "watchkeeper_trust_boundary",
                    test_name,
                    "--",
                    "--ignored",
                    "--exact",
                ),
                test_name,
            )
            for test_name in (
                "live_docker_public_job_completes_deterministic_gate_and_cleans_daemon_state",
                "live_docker_public_cancel_removes_container_record_and_prevents_retry",
                "live_docker_worker_sigkill_reconciles_stale_container_before_one_retry",
            )
        ),
        "This uses real local Docker with credential-free smoke transports. It proves public deterministic containment, cancellation and crash recovery, not live provider execution or semantic achievement.",
        public_docker_required=True,
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
        "Supported run, orchestration, stored-plan fork, chain and campaign creation enter one durable Job lifecycle and five-command journey with approved authority provenance; retired public resume and stored-chain execution or mutation refuse without changing state.",
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
                    "commands::run::durable_direct_tests::guided_continuation_preserves_approved_authority_provenance",
                    "--",
                    "--exact",
                ),
                "commands::run::durable_direct_tests::guided_continuation_preserves_approved_authority_provenance",
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
                    "commands::chain::tests::durable_chain_freezes_one_graph_job_with_isolated_per_node_delivery",
                    "--",
                    "--exact",
                ),
                "commands::chain::tests::durable_chain_freezes_one_graph_job_with_isolated_per_node_delivery",
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
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--test",
                    "lifecycle",
                    "public_resume_of_unowned_legacy_run_refuses_without_state_mutation",
                    "--",
                    "--exact",
                ),
                "public_resume_of_unowned_legacy_run_refuses_without_state_mutation",
            ),
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--test",
                    "chain",
                    "--features",
                    "internal-characterization",
                    "product_chain_run_refuses_without_mutating_the_stored_chain",
                    "--",
                    "--exact",
                ),
                "product_chain_run_refuses_without_mutating_the_stored_chain",
            ),
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--test",
                    "chain",
                    "--features",
                    "internal-characterization",
                    "product_chain_extend_refuses_without_mutation_and_preserves_the_requested_goal",
                    "--",
                    "--exact",
                ),
                "product_chain_extend_refuses_without_mutation_and_preserves_the_requested_goal",
            ),
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--test",
                    "chain",
                    "--features",
                    "internal-characterization",
                    "product_chain_redo_extend_refuses_before_state_or_event_mutation",
                    "--",
                    "--exact",
                ),
                "product_chain_redo_extend_refuses_before_state_or_event_mutation",
            ),
        ),
        "This proves the supported creation routes, immutable approval provenance and refusal boundaries; historical artifacts remain readable compatibility surfaces, not trusted execution routes.",
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
        "Graph and Campaign semantic revise decisions run bounded, fenced parent-only repairs, recover candidate-ready work within one durable deadline and preserve one parent Job identity.",
        "hermetic_parent_repair",
        (
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon-core",
                    "--lib",
                    "job_lease::tests::fenced_json_authority_is_created_exactly_once",
                    "--",
                    "--exact",
                ),
                "job_lease::tests::fenced_json_authority_is_created_exactly_once",
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
                    "merge_repair_adoption_deadline_is_stable_and_bound_to_its_window",
                    "--",
                    "--exact",
                ),
                "merge_repair_adoption_deadline_is_stable_and_bound_to_its_window",
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
                    "legacy_pending_repair_deadline_is_anchored_to_original_preparation",
                    "--",
                    "--exact",
                ),
                "legacy_pending_repair_deadline_is_anchored_to_original_preparation",
            ),
            ProofCommand(
                (
                    "cargo",
                    "test",
                    "-p",
                    "deadreckon",
                    "--test",
                    "watchkeeper_repair_child_ownership",
                    "graph_and_campaign_adopt_final_budget_repair_after_driver_crash",
                    "--",
                    "--exact",
                ),
                "graph_and_campaign_adopt_final_budget_repair_after_driver_crash",
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
        "This uses scripted providers, real process interruption and persisted crash fixtures; a naturally occurring live provider revise remains an operator trial.",
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
        "id": "live_docker_gate_attack",
        "status": "unproven",
        "reason": "requires distinct approved live hostile-worker and independent-judge routes plus a valid Docker-bound completion receipt",
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
    lines = [line.strip() for line in (stdout + "\n" + stderr).splitlines()]
    matching = [
        line.strip()
        for line in lines
        if proof.expected_test in line or line.startswith("test result:")
    ]
    named_test_started = any(proof.expected_test in line for line in matching)
    named_test_passed_inline = any(
        proof.expected_test in line and "ok" in line for line in matching
    )
    successful_exact_summary = any(
        line.startswith("test result: ok.")
        and "1 passed" in line
        and "0 failed" in line
        for line in matching
    )
    # Some live subprocesses inherit the libtest output descriptor and can
    # print while libtest is holding `test <name> ... ` on the same line. In
    # that case the final `ok` appears separately. The command selects exactly
    # one named test, so the named start plus Cargo's one-pass/zero-fail summary
    # is the same proof without depending on line-buffering behavior.
    observed_pass = (
        completed.returncode == 0
        and named_test_started
        and (named_test_passed_inline or successful_exact_summary)
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
        (
            binary,
            "image",
            "inspect",
            "rust:1",
            "--format",
            "{{.Id}} {{.Architecture}}/{{.Os}}",
        ),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    image_fields = image.stdout.decode("utf-8", errors="replace").strip().split()
    image_id = image_fields[0] if image.returncode == 0 and image_fields else None
    image_platform = image_fields[1] if len(image_fields) == 2 else None
    image_cached = image.returncode == 0 and image_id is not None
    operational = daemon.returncode == 0 and image_cached
    return {
        "available": True,
        "operational": operational,
        "image_cached": image_cached,
        "image_id_sha256": digest(image_id.encode()) if image_id is not None else None,
        "image_platform": image_platform,
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


def cargo_target_directory(repo: Path) -> Path:
    configured = os.environ.get("CARGO_TARGET_DIR")
    if configured is None:
        return repo / "target"
    target = Path(configured)
    return target if target.is_absolute() else repo / target


def static_linux_arm64_elf(data: bytes) -> bool:
    if len(data) < 64 or data[:6] != b"\x7fELF\x02\x01":
        return False
    try:
        machine = struct.unpack_from("<H", data, 18)[0]
        program_offset = struct.unpack_from("<Q", data, 32)[0]
        entry_size = struct.unpack_from("<H", data, 54)[0]
        entry_count = struct.unpack_from("<H", data, 56)[0]
    except struct.error:
        return False
    if machine != 183 or program_offset < 64 or entry_size < 56 or entry_count == 0:
        return False
    if program_offset + entry_size * entry_count > len(data):
        return False
    for index in range(entry_count):
        start = program_offset + index * entry_size
        if struct.unpack_from("<I", data, start)[0] == 3:
            return False
    return True


def source_bundle_build_id(repo: Path) -> tuple[str | None, str | None]:
    node = shutil.which("node")
    trust = repo / "release" / "trust" / "release-trust.mjs"
    if node is None or not trust.is_file():
        return None, "Node or the source-bundle verifier is unavailable"
    completed = subprocess.run(
        (node, str(trust), "source-bundle-id", "--root", str(repo), "--raw"),
        cwd=repo,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    value = completed.stdout.decode("utf-8", errors="replace").strip()
    if (
        completed.returncode != 0
        or BUNDLE_BUILD_ID_PATTERN.fullmatch(value.encode()) is None
    ):
        return None, "the source-bundle verifier did not return one valid identity"
    return value, None


def public_docker_preflight(repo: Path, docker: dict[str, Any]) -> dict[str, Any]:
    sidecar_name = "dr-gate-evaluator-aarch64-unknown-linux-musl"
    sidecar = cargo_target_directory(repo) / "debug" / sidecar_name
    sidecar_present = False
    sidecar_regular = False
    sidecar_executable = False
    sidecar_compatible = False
    sidecar_sha256 = None
    sidecar_bundle_ids: list[str] = []
    try:
        metadata = sidecar.lstat()
        sidecar_present = True
        sidecar_regular = stat.S_ISREG(metadata.st_mode)
        sidecar_executable = sidecar_regular and bool(metadata.st_mode & 0o111)
        if sidecar_regular and 0 < metadata.st_size <= 256 * 1024 * 1024:
            sidecar_bytes = sidecar.read_bytes()
            sidecar_sha256 = digest(sidecar_bytes)
            sidecar_compatible = static_linux_arm64_elf(sidecar_bytes)
            sidecar_bundle_ids = sorted(
                {
                    match.decode("ascii")
                    for match in BUNDLE_BUILD_ID_PATTERN.findall(sidecar_bytes)
                }
            )
    except OSError:
        pass

    image_compatible = docker.get("image_platform") == "arm64/linux"
    source_bundle_id, source_bundle_error = source_bundle_build_id(repo)
    bundle_compatible = (
        len(sidecar_bundle_ids) == 1
        and source_bundle_id is not None
        and sidecar_bundle_ids[0] == source_bundle_id
    )
    operational = (
        bool(docker.get("operational"))
        and image_compatible
        and sidecar_regular
        and sidecar_executable
        and sidecar_compatible
        and bundle_compatible
    )
    if not docker.get("operational"):
        reason = docker.get("reason")
    elif not image_compatible:
        reason = "the cached rust:1 image is not arm64/linux"
    elif not sidecar_present:
        reason = (
            "the static Linux arm64 evaluator sidecar is not installed beside "
            "Cargo's debug deadreckon binary"
        )
    elif not sidecar_regular:
        reason = "the static Linux arm64 evaluator sidecar is not a regular non-symlink file"
    elif not sidecar_executable:
        reason = "the static Linux arm64 evaluator sidecar is not executable"
    elif not sidecar_compatible:
        reason = "the evaluator sidecar is not a static Linux arm64 ELF binary"
    elif len(sidecar_bundle_ids) != 1:
        reason = "the evaluator sidecar does not carry exactly one DeadReckon build-bundle identity"
    elif source_bundle_error is not None:
        reason = source_bundle_error
    elif not bundle_compatible:
        reason = "the evaluator sidecar belongs to a different DeadReckon build bundle than the clean source"
    else:
        reason = None
    return {
        "operational": operational,
        "image_compatible": image_compatible,
        "sidecar_name": sidecar_name,
        "sidecar_present": sidecar_present,
        "sidecar_regular": sidecar_regular,
        "sidecar_executable": sidecar_executable,
        "sidecar_compatible": sidecar_compatible,
        "sidecar_sha256": sidecar_sha256,
        "sidecar_bundle_id": (
            sidecar_bundle_ids[0] if len(sidecar_bundle_ids) == 1 else None
        ),
        "source_bundle_id": source_bundle_id,
        "bundle_compatible": bundle_compatible,
        "reason": reason,
    }


def run_trial(
    repo: Path,
    trial: Trial,
    seatbelt: dict[str, Any],
    docker: dict[str, Any],
    public_docker: dict[str, Any],
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
    if trial.public_docker_required and not public_docker["operational"]:
        return {
            "id": trial.trial_id,
            "claim": trial.claim,
            "proof_type": trial.proof_type,
            "status": "unproven",
            "reason": public_docker["reason"],
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
    public_docker = public_docker_preflight(repo, docker)
    trials = [run_trial(repo, trial, seatbelt, docker, public_docker) for trial in selected]
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
            "public_docker_preflight": public_docker,
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
