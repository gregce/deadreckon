#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

fn dogfood_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/watchkeeper-dogfood")
}

fn git_output(repo: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("run git {args:?} in {}: {error}", repo.display()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn validate_credential_free_source(payload: &Value, repo: &Path) -> Result<String, String> {
    match payload["repository"]["dirty"].as_bool() {
        Some(false) => {}
        Some(true) => {
            return Err(
                "checked credential-free evidence was generated from a dirty repository"
                    .to_string(),
            );
        }
        None => {
            return Err(
                "checked credential-free evidence has no boolean repository.dirty".to_string(),
            );
        }
    }

    let revision = payload["repository"]["revision"]
        .as_str()
        .filter(|revision| {
            matches!(revision.len(), 40 | 64)
                && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .ok_or_else(|| {
            "checked credential-free evidence has no full hexadecimal source revision".to_string()
        })?;
    let commit = format!("{revision}^{{commit}}");
    let exists = git_output(repo, &["rev-parse", "--verify", &commit]);
    if !exists.status.success() {
        let shallow = git_output(repo, &["rev-parse", "--is-shallow-repository"]);
        if !shallow.status.success() || String::from_utf8_lossy(&shallow.stdout).trim() != "true" {
            return Err(format!(
                "checked credential-free source revision {revision} is not present in the repository"
            ));
        }
        return Ok(revision.to_string());
    }

    let ancestor = git_output(repo, &["merge-base", "--is-ancestor", revision, "HEAD"]);
    if !ancestor.status.success() {
        return Err(format!(
            "checked credential-free source revision {revision} is not an ancestor of HEAD"
        ));
    }
    Ok(revision.to_string())
}

#[test]
fn dogfood_harness_uses_public_start_status_finish_and_receipt() {
    let source = fs::read_to_string(dogfood_dir().join("run.sh")).expect("dogfood harness");
    let start = source
        .find("\"$deadreckon_bin\" start")
        .expect("public start");
    let status = source
        .find("\"$deadreckon_bin\" status")
        .expect("public status");
    let report = source
        .find("\"$deadreckon_bin\" report")
        .expect("public report");
    let receipt = source
        .find("jobs/$job_id/receipt.json")
        .expect("durable receipt");
    let finish = source
        .find("\"$deadreckon_bin\" finish")
        .expect("public finish");

    assert!(start < status);
    assert!(status < report);
    assert!(report < receipt);
    assert!(receipt < finish);
    assert!(source.contains("DEADRECKON_DOGFOOD_EXECUTE"));
    assert!(source.contains("--quiet"));
    assert!(source.contains("\"contained\": True"));
    assert!(source.contains("\"proof_kind\": \"two_key_completion\""));
    assert!(source.contains("\"terminal_outcome\": projection.get(\"outcome\")"));
    assert!(source.contains("\"receipt_validation_source\""));
    assert!(source.contains("\"receipt_validation_exit_status\""));
    assert!(source.contains("\"finish_exit_status\""));
}

#[test]
fn watchkeeper_provenance_jobs_checkout_full_history() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let ci = fs::read_to_string(repo.join(".github/workflows/ci.yml")).expect("CI workflow");
    let ci_test_job = ci
        .split_once("  test:\n")
        .and_then(|(_, jobs)| jobs.split_once("\n  release-zip-smoke:"))
        .map(|(job, _)| job)
        .expect("CI test job");
    assert!(
        ci_test_job.contains("fetch-depth: 0"),
        "the Watchkeeper provenance test needs Git history in branch CI"
    );

    let release =
        fs::read_to_string(repo.join(".github/workflows/release.yml")).expect("release workflow");
    let release_verify_job = release
        .split_once("  release-verify:\n")
        .and_then(|(_, jobs)| jobs.split_once("\n  build-evaluator-sidecars:"))
        .map(|(job, _)| job)
        .expect("release verify job");
    assert!(
        release_verify_job.contains("fetch-depth: 0"),
        "the Watchkeeper provenance test needs Git history in release verification"
    );
}

#[test]
fn adversarial_runner_names_each_boundary_and_keeps_live_claims_unproven() {
    let source =
        fs::read_to_string(dogfood_dir().join("adversarial.py")).expect("adversarial runner");
    for trial in [
        "terminal_detach",
        "worker_kill",
        "supervisor_restart",
        "network_denial",
        "macos_developer_tool_gate",
        "gate_key_search_and_forgery",
        "docker_control_boundary",
        "docker_gate_boundary",
        "receipt_mutation",
        "result_delivery",
        "unified_job_journey",
        "job_child_and_planner_boundaries",
        "semantic_parent_repair",
        "repair_lineage_tamper",
    ] {
        assert!(source.contains(trial), "missing adversarial trial {trial}");
    }
    for unproven in [
        "live_provider_worker_kill",
        "live_provider_supervisor_restart",
        "live_provider_network_loss",
        "machine_reboot",
        "cross_provider_gate_attack",
        "live_provider_parent_repair",
        "live_campaign_interruption_recovery",
        "linux_bubblewrap_gate_boundary",
        "live_docker_gate_attack",
    ] {
        assert!(
            source.contains(unproven),
            "missing live boundary {unproven}"
        );
    }
    let schema: Value = serde_json::from_slice(
        &fs::read(dogfood_dir().join("adversarial.schema.json")).expect("adversarial schema"),
    )
    .expect("adversarial schema JSON");
    let schema_live_ids = schema["properties"]["live_claims"]["items"]["properties"]["id"]["enum"]
        .as_array()
        .expect("live claim ID enum")
        .iter()
        .map(|value| value.as_str().expect("live claim ID"))
        .collect::<BTreeSet<_>>();
    let expected_live_ids = [
        "live_provider_worker_kill",
        "live_provider_supervisor_restart",
        "live_provider_network_loss",
        "machine_reboot",
        "cross_provider_gate_attack",
        "live_provider_parent_repair",
        "live_campaign_interruption_recovery",
        "linux_bubblewrap_gate_boundary",
        "live_docker_gate_attack",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    assert_eq!(schema_live_ids, expected_live_ids);
    for unified_job_proof in [
        "guided_continuation_preserves_approved_authority_provenance",
        "durable_chain_freezes_one_graph_job_with_isolated_per_node_delivery",
        "public_resume_of_unowned_legacy_run_refuses_without_state_mutation",
        "product_chain_run_refuses_without_mutating_the_stored_chain",
        "product_chain_extend_refuses_without_mutation_and_preserves_the_requested_goal",
        "product_chain_redo_extend_refuses_before_state_or_event_mutation",
    ] {
        assert!(
            source.contains(unified_job_proof),
            "missing unified Job proof {unified_job_proof}"
        );
    }
    for public_docker_proof in [
        "live_docker_public_job_completes_deterministic_gate_and_cleans_daemon_state",
        "live_docker_public_cancel_removes_container_record_and_prevents_retry",
        "live_docker_worker_sigkill_reconciles_stale_container_before_one_retry",
    ] {
        assert!(
            source.contains(public_docker_proof),
            "missing public Docker proof {public_docker_proof}"
        );
    }
    assert!(source.contains("\"status\": \"unproven\""));
    assert!(source.contains("\"matrix_status\": matrix_status(repo)"));
    assert!(source.contains("\"seatbelt_preflight\": seatbelt"));
    assert!(source.contains("\"docker_preflight\": docker"));
    assert!(source.contains("\"public_docker_preflight\": public_docker"));
    assert!(source.contains("DEADRECKON_LIVE_DOCKER_TEST=1"));
    assert!(source.contains("dr-gate-evaluator-aarch64-unknown-linux-musl"));
    assert!(source.contains("arm64/linux"));
    assert!(source.contains("bundle_compatible"));
}

#[test]
fn public_docker_preflight_gates_prerequisites_and_runs_all_three_proofs_when_ready() {
    let temp = TempDir::new().expect("tempdir");
    let fake_bin = temp.path().join("bin");
    let target = temp.path().join("target");
    fs::create_dir_all(&fake_bin).expect("fake bin");
    let docker = fake_bin.join("docker");
    fs::write(
        &docker,
        r#"#!/bin/sh
case "$1" in
  version) printf '%s\n' '27.0.0' ;;
  image) printf 'sha256:fixture %s\n' "${WK_FAKE_DOCKER_PLATFORM:-arm64/linux}" ;;
  *) exit 2 ;;
esac
"#,
    )
    .expect("fake docker");
    fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).expect("docker executable");
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runner = dogfood_dir().join("adversarial.py");
    let run = |name: &str, platform: &str| {
        let output = temp.path().join(format!("{name}.json"));
        let status = Command::new("python3")
            .arg(&runner)
            .arg("--repo")
            .arg(&repo)
            .arg("--output")
            .arg(&output)
            .args(["--only", "docker_gate_boundary"])
            .env("PATH", &path)
            .env("CARGO_TARGET_DIR", &target)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("WK_FAKE_DOCKER_PLATFORM", platform)
            .status()
            .expect("credential-free runner");
        assert!(status.success(), "an unavailable prerequisite is unproven");
        serde_json::from_slice::<Value>(&fs::read(output).expect("runner output"))
            .expect("runner JSON")
    };

    let wrong_image = run("wrong-image", "amd64/linux");
    assert_eq!(wrong_image["trials"][0]["status"], "unproven");
    assert_eq!(
        wrong_image["trials"][0]["reason"],
        "the cached rust:1 image is not arm64/linux"
    );
    assert_eq!(wrong_image["trials"][0]["commands"], json!([]));

    let missing_sidecar = run("missing-sidecar", "arm64/linux");
    assert_eq!(missing_sidecar["trials"][0]["status"], "unproven");
    assert!(
        missing_sidecar["trials"][0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("sidecar is not installed"))
    );

    let sidecar = target
        .join("debug")
        .join("dr-gate-evaluator-aarch64-unknown-linux-musl");
    fs::create_dir_all(sidecar.parent().expect("sidecar parent")).expect("sidecar parent");
    fs::write(&sidecar, "not an ELF executable").expect("invalid sidecar");
    fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o755)).expect("sidecar executable");
    let dynamic_or_invalid = run("invalid-sidecar", "arm64/linux");
    assert_eq!(dynamic_or_invalid["trials"][0]["status"], "unproven");
    assert_eq!(
        dynamic_or_invalid["trials"][0]["reason"],
        "the evaluator sidecar is not a static Linux arm64 ELF binary"
    );
    assert_eq!(
        dynamic_or_invalid["live_claims"].as_array().map(Vec::len),
        Some(9)
    );
    assert!(
        dynamic_or_invalid["live_claims"]
            .as_array()
            .is_some_and(|claims| {
                claims
                    .iter()
                    .all(|claim| claim["id"] != "docker_gate_boundary")
            })
    );
    assert!(
        dynamic_or_invalid["live_claims"]
            .as_array()
            .is_some_and(|claims| claims
                .iter()
                .any(|claim| claim["id"] == "live_docker_gate_attack"))
    );

    let cargo = fake_bin.join("cargo");
    fs::write(
        &cargo,
        r#"#!/bin/sh
for argument in "$@"; do
  case "$argument" in
    live_docker_*)
      printf 'test %s ... 28.0.4\n' "$argument"
      printf 'ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 8 filtered out\n'
      exit 0
      ;;
  esac
done
exit 2
"#,
    )
    .expect("fake cargo");
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).expect("cargo executable");
    let static_arm64_elf = |bundle_id: &str| {
        let mut bytes = vec![0_u8; 120];
        bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
        bytes[18..20].copy_from_slice(&183_u16.to_le_bytes());
        bytes[32..40].copy_from_slice(&64_u64.to_le_bytes());
        bytes[54..56].copy_from_slice(&56_u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&1_u16.to_le_bytes());
        bytes[64..68].copy_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(bundle_id.as_bytes());
        bytes
    };
    let mismatched_bundle = format!("deadreckon-bundle-build-id-sha256:{}", "0".repeat(64));
    fs::write(&sidecar, static_arm64_elf(&mismatched_bundle)).expect("mismatched sidecar");
    let mismatched = run("mismatched-sidecar", "arm64/linux");
    assert_eq!(mismatched["trials"][0]["status"], "unproven");
    assert_eq!(
        mismatched["trials"][0]["reason"],
        "the evaluator sidecar belongs to a different DeadReckon build bundle than the clean source"
    );
    assert_eq!(
        mismatched["host"]["public_docker_preflight"]["bundle_compatible"],
        false
    );

    let source_bundle = Command::new("node")
        .arg(repo.join("release/trust/release-trust.mjs"))
        .args(["source-bundle-id", "--root"])
        .arg(&repo)
        .arg("--raw")
        .output()
        .expect("source bundle command");
    assert!(source_bundle.status.success());
    let source_bundle = String::from_utf8(source_bundle.stdout)
        .expect("source bundle UTF-8")
        .trim()
        .to_string();
    fs::write(&sidecar, static_arm64_elf(&source_bundle)).expect("static arm64 sidecar");
    let ready = run("ready", "arm64/linux");
    assert_eq!(ready["trials"][0]["status"], "passed");
    assert_eq!(
        ready["host"]["public_docker_preflight"]["bundle_compatible"],
        true
    );
    let commands = ready["trials"][0]["commands"]
        .as_array()
        .expect("public Docker commands");
    assert_eq!(commands.len(), 3);
    assert!(
        commands
            .iter()
            .all(|command| command["observed_pass"] == true)
    );
}

#[test]
fn live_fault_trials_are_operator_gated_objective_and_sanitized() {
    let expected = [
        "live_provider_worker_kill",
        "live_provider_supervisor_restart",
        "live_provider_network_loss",
        "machine_reboot",
        "cross_provider_gate_attack",
        "live_provider_parent_repair",
        "live_campaign_interruption_recovery",
        "linux_bubblewrap_gate_boundary",
        "live_docker_gate_attack",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    let manifest: Value = serde_json::from_slice(
        &fs::read(dogfood_dir().join("live-trials.json")).expect("live trial manifest"),
    )
    .expect("live trial manifest JSON");
    let claims = manifest["claim_ids"]
        .as_array()
        .expect("live claim ids")
        .iter()
        .map(|id| id.as_str().expect("live claim id"))
        .collect::<BTreeSet<_>>();
    assert_eq!(claims, expected);

    let trials = manifest["trials"].as_array().expect("live trials");
    assert_eq!(trials.len(), expected.len());
    let supported_oracles = [
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
        "network_connectivity_transition_bound",
        "sandbox_boundary_observation_bound",
        "structurally_inconclusive",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    let supported_sources = [
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
        "network-connectivity-observation",
        "unavailable-objective",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    for trial in trials {
        let id = trial["id"].as_str().expect("live trial id");
        assert!(expected.contains(id), "unexpected live trial {id}");
        assert_eq!(
            trial["intervention"]["operator_only"], true,
            "{id} may only be initiated by an operator"
        );
        assert!(
            !trial["prerequisites"]
                .as_array()
                .expect("prerequisites")
                .is_empty(),
            "{id} has no prerequisites"
        );
        assert!(
            !trial["evidence"].as_array().expect("evidence").is_empty(),
            "{id} has no objective evidence"
        );
        for evidence in trial["evidence"].as_array().expect("evidence") {
            let source = evidence["source"]
                .as_str()
                .expect("trusted evidence source");
            assert!(
                supported_sources.contains(source),
                "{id} uses unsupported trusted source {source}"
            );
        }
        assert!(
            !trial["cleanup"].as_array().expect("cleanup").is_empty(),
            "{id} has no cleanup instructions"
        );
        for oracle in trial["oracles"].as_array().expect("oracles") {
            let kind = oracle["type"].as_str().expect("oracle type");
            assert!(
                supported_oracles.contains(kind),
                "{id} uses unsupported oracle {kind}"
            );
        }
    }
    for id in [
        "cross_provider_gate_attack",
        "linux_bubblewrap_gate_boundary",
        "live_docker_gate_attack",
    ] {
        let trial = trials
            .iter()
            .find(|trial| trial["id"] == id)
            .expect("hostile gate trial");
        assert!(
            trial["oracles"]
                .as_array()
                .expect("oracles")
                .iter()
                .any(|oracle| oracle["id"] == "receipt_valid"
                    && oracle["evidence"] == "job-report"
                    && oracle["expected"] == "valid"),
            "{id} trusts worker observation without a valid public receipt"
        );
        assert!(
            trial["oracles"]
                .as_array()
                .expect("oracles")
                .iter()
                .any(|oracle| oracle["id"] == "authoritative_attack_observation"
                    && oracle["type"] == "sandbox_boundary_observation_bound"
                    && oracle["authority_evidence"] == "authority"
                    && oracle["job_evidence"] == "job-view-after"
                    && oracle["events_evidence"] == "events-after"
                    && oracle["report_evidence"] == "job-report"),
            "{id} does not bind the authenticated boundary observation"
        );
    }
    let reboot = trials
        .iter()
        .find(|trial| trial["id"] == "machine_reboot")
        .expect("reboot trial");
    assert!(
        reboot["oracles"]
            .as_array()
            .expect("reboot oracles")
            .iter()
            .any(|oracle| oracle["type"] == "event_suffix_order"
                && oracle["before"] == "lease_reclaimed"
                && oracle["after"] == "attempt_started"),
        "reboot evidence must prove execution resumed after reclaim"
    );

    let schema: Value = serde_json::from_slice(
        &fs::read(dogfood_dir().join("live-trial-results.schema.json"))
            .expect("live trial result schema"),
    )
    .expect("live trial result schema JSON");
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["properties"]["sanitized"]["const"], true);
    assert_eq!(
        schema["properties"]["status"]["enum"],
        json!(["not_run", "passed", "failed", "inconclusive"])
    );
    assert_eq!(
        schema["allOf"][0]["then"]["properties"]["intervention"]["properties"]["status"]["const"],
        "performed"
    );
    assert_eq!(
        schema["allOf"][0]["then"]["properties"]["cleanup"]["properties"]["status"]["const"],
        "completed"
    );

    let recorder =
        fs::read_to_string(dogfood_dir().join("live-trial.py")).expect("live trial recorder");
    for forbidden in [
        "import signal",
        "import socket",
        "import requests",
        "import urllib",
    ] {
        assert!(
            !recorder.contains(forbidden),
            "passive recorder contains {forbidden}"
        );
    }
    assert!(recorder.contains("import subprocess"));
    assert_eq!(
        recorder.matches("subprocess.run(").count(),
        1,
        "the passive recorder may execute only its pinned capture-helper seam"
    );
    assert!(recorder.contains("[str(helper), *command]"));
    assert!(
        recorder.contains(
            "stable_regular_path(Path(str(capture.get(\"helper\"))), \"capture helper\")"
        )
    );
    assert!(recorder.contains("cleanup != \"completed\""));
    assert!(recorder.contains("captured evidence must remain a regular non-symlink file"));
    assert!(!recorder.contains("attack-observation"));
    assert!(!recorder.contains("campaign-recovery-summary"));
}

#[test]
fn checked_adversarial_results_match_the_runner_and_have_no_false_live_claim() {
    let result_path = dogfood_dir().join("credential-free-results.json");
    if !result_path.is_file() {
        return;
    }
    let matrix: Value =
        serde_json::from_slice(&fs::read(dogfood_dir().join("matrix.json")).expect("matrix"))
            .expect("matrix JSON");
    let tasks = matrix["tasks"].as_array().expect("matrix tasks");
    let expected_counts = tasks.iter().fold(
        std::collections::BTreeMap::<&str, u64>::new(),
        |mut counts, task| {
            let status = task["execution_status"]
                .as_str()
                .expect("matrix execution status");
            *counts.entry(status).or_default() += 1;
            counts
        },
    );
    let payload: Value =
        serde_json::from_slice(&fs::read(&result_path).expect("results")).expect("results JSON");
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["credential_free"], true);
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source_revision =
        validate_credential_free_source(&payload, &repo).unwrap_or_else(|error| panic!("{error}"));
    let expected_runner_sha = payload["runner_sha256"]
        .as_str()
        .expect("credential-free runner digest");
    let current_runner = fs::read(dogfood_dir().join("adversarial.py")).expect("current runner");
    let source_runner = if sha256_bytes(&current_runner) == expected_runner_sha {
        current_runner
    } else {
        let runner_object =
            format!("{source_revision}:examples/watchkeeper-dogfood/adversarial.py");
        let historical = git_output(&repo, &["show", &runner_object]);
        assert!(
            historical.status.success(),
            "credential-free runner provenance is unprovable: current bytes do not match and the historical source runner is unavailable"
        );
        assert_eq!(
            sha256_bytes(&historical.stdout),
            expected_runner_sha,
            "credential-free runner provenance is invalid: neither current nor historical source bytes match the recorded digest"
        );
        historical.stdout
    };
    assert_eq!(payload["matrix_status"]["total_tasks"], tasks.len());
    for (status, count) in expected_counts {
        assert_eq!(
            payload["matrix_status"]["by_execution_status"][status],
            count
        );
    }
    assert_eq!(payload["summary"]["failed"], 0);
    assert_eq!(payload["runner_sha256"], sha256_bytes(&source_runner));
    let trials = payload["trials"]
        .as_array()
        .expect("credential-free trials");
    let source_runner_text = String::from_utf8(source_runner).expect("source runner UTF-8");
    let includes_macos_developer_tool_gate =
        source_runner_text.contains("macos_developer_tool_gate");
    let mut expected_trials = vec![
        "terminal_detach",
        "worker_kill",
        "supervisor_restart",
        "network_denial",
        "gate_key_search_and_forgery",
        "docker_control_boundary",
        "docker_gate_boundary",
        "receipt_mutation",
        "result_delivery",
        "unified_job_journey",
        "job_child_and_planner_boundaries",
        "semantic_parent_repair",
        "repair_lineage_tamper",
    ];
    if includes_macos_developer_tool_gate {
        expected_trials.insert(4, "macos_developer_tool_gate");
    }
    assert_eq!(trials.len(), expected_trials.len());
    for id in expected_trials {
        let trial = trials
            .iter()
            .find(|trial| trial["id"].as_str() == Some(id))
            .unwrap_or_else(|| panic!("missing checked adversarial result {id}"));
        assert_eq!(
            trial["status"], "passed",
            "checked adversarial result {id} must pass"
        );
    }
    let public_docker = trials
        .iter()
        .find(|trial| trial["id"] == "docker_gate_boundary")
        .expect("public Docker result");
    let public_docker_commands = public_docker["commands"]
        .as_array()
        .expect("public Docker commands");
    assert_eq!(public_docker_commands.len(), 3);
    for (command, expected) in public_docker_commands.iter().zip([
        "live_docker_public_job_completes_deterministic_gate_and_cleans_daemon_state",
        "live_docker_public_cancel_removes_container_record_and_prevents_retry",
        "live_docker_worker_sigkill_reconciles_stale_container_before_one_retry",
    ]) {
        assert_eq!(command["expected_test"], expected);
        assert_eq!(command["observed_pass"], true);
    }
    assert_eq!(
        payload["host"]["public_docker_preflight"]["operational"],
        true
    );
    let live = payload["live_claims"].as_array().expect("live claims");
    let split_live_docker_claim = source_runner_text.contains("live_docker_gate_attack");
    assert_eq!(live.len(), if split_live_docker_claim { 9 } else { 8 });
    assert!(
        live.iter()
            .all(|claim| claim["status"].as_str() == Some("unproven"))
    );
    assert!(
        live.iter()
            .all(|claim| claim["id"].as_str() != Some("docker_gate_boundary"))
    );
    assert_eq!(
        live.iter()
            .any(|claim| claim["id"].as_str() == Some("live_docker_gate_attack")),
        split_live_docker_claim
    );
}

#[test]
fn dirty_credential_free_evidence_is_rejected() {
    let payload = json!({
        "repository": {
            "revision": "0123456789012345678901234567890123456789",
            "dirty": true
        }
    });

    let error = validate_credential_free_source(&payload, Path::new("."))
        .expect_err("dirty evidence must be rejected");
    assert!(error.contains("generated from a dirty repository"));
}

#[test]
fn clean_source_evidence_remains_valid_in_a_descendant_commit() {
    let temp = TempDir::new().expect("tempdir");
    let repo = temp.path();
    for args in [
        &["init", "-q"][..],
        &["config", "user.email", "watchkeeper@example.invalid"][..],
        &["config", "user.name", "Watchkeeper fixture"][..],
        &["config", "commit.gpgsign", "false"][..],
    ] {
        let output = git_output(repo, args);
        assert!(output.status.success(), "git {args:?} failed");
    }
    fs::write(repo.join("source"), "source\n").expect("source fixture");
    assert!(git_output(repo, &["add", "source"]).status.success());
    assert!(
        git_output(repo, &["commit", "-q", "-m", "source"])
            .status
            .success()
    );
    let source = String::from_utf8(git_output(repo, &["rev-parse", "HEAD"]).stdout)
        .expect("UTF-8 revision")
        .trim()
        .to_string();
    fs::write(repo.join("evidence"), "evidence\n").expect("evidence fixture");
    assert!(git_output(repo, &["add", "evidence"]).status.success());
    assert!(
        git_output(repo, &["commit", "-q", "-m", "evidence"])
            .status
            .success()
    );
    let payload = json!({
        "repository": {
            "revision": source,
            "dirty": false
        }
    });

    assert_eq!(
        validate_credential_free_source(&payload, repo).expect("clean ancestor source"),
        payload["repository"]["revision"]
    );
}

#[test]
fn dogfood_matrix_and_sanitized_results_agree() {
    let raw = fs::read_to_string(dogfood_dir().join("matrix.json")).expect("matrix");
    let matrix: Value = serde_json::from_str(&raw).expect("valid matrix JSON");
    let tasks = matrix["tasks"].as_array().expect("tasks");
    let repository_slots = matrix["repositories"].as_array().expect("repositories");
    let provider_slots = matrix["providers"].as_array().expect("providers");
    let used_repositories = tasks
        .iter()
        .filter_map(|task| task["repository"].as_str())
        .collect::<BTreeSet<_>>();
    let used_providers = tasks
        .iter()
        .filter_map(|task| task["provider"].as_str())
        .collect::<BTreeSet<_>>();

    assert!(
        (20..=30).contains(&tasks.len()),
        "task count: {}",
        tasks.len()
    );
    assert!(repository_slots.len() >= 2);
    assert!(provider_slots.len() >= 2);
    assert!(used_repositories.len() >= 2);
    assert!(used_providers.len() >= 2);
    let attempted = tasks
        .iter()
        .filter(|task| task["execution_status"] == "attempted")
        .collect::<Vec<_>>();
    let not_run = tasks
        .iter()
        .filter(|task| task["execution_status"] == "not_run")
        .count();
    assert!(
        attempted.len() >= 2,
        "the checked matrix must retain the live attempts already performed"
    );
    assert_eq!(attempted.len() + not_run, tasks.len());
    assert_eq!(matrix["execution_policy"], "operator_only");

    let results_raw =
        fs::read_to_string(dogfood_dir().join("trial-results.json")).expect("trial results");
    assert!(!results_raw.contains("/Users/"), "absolute path leaked");
    let results: Value = serde_json::from_str(&results_raw).expect("trial results JSON");
    assert_eq!(results["sanitized"], true);
    assert_eq!(results["summary"]["total_tasks"], tasks.len());
    assert_eq!(results["summary"]["attempted"], attempted.len());
    assert_eq!(results["summary"]["not_run"], not_run);
    let result_entries = results["results"].as_object().expect("result entries");
    assert_eq!(result_entries.len(), attempted.len());
    let verified = result_entries
        .values()
        .filter(|result| {
            result["terminal"]["outcome"] == "verified" && result["receipt_present"] == true
        })
        .count();
    assert_eq!(results["summary"]["verified"], verified);
    for task in attempted {
        let id = task["id"].as_str().expect("attempted task id");
        assert_eq!(
            task["result_ref"],
            format!("trial-results.json#/results/{id}")
        );
        assert_eq!(results["results"][id]["execution_status"], "attempted");
        assert!(results["results"][id]["receipt_present"].is_boolean());
        assert!(results["results"][id]["finish_attempted"].is_boolean());
        if results["results"][id]["receipt_present"] == true {
            assert_eq!(results["results"][id]["terminal"]["outcome"], "verified");
        }
        assert_eq!(
            results["results"][id]["job_id_prefix"]
                .as_str()
                .expect("sanitized job prefix")
                .len(),
            8
        );
    }
}

#[test]
fn failed_terminal_job_leaves_an_operator_observation_before_receipt_refusal() {
    let temp = TempDir::new().expect("tempdir");
    let repository = temp.path().join("repo");
    let home = temp.path().join("home");
    let artifacts = temp.path().join("artifacts");
    let fake_deadreckon = temp.path().join("deadreckon");
    let matrix = temp.path().join("matrix.json");
    fs::create_dir_all(&repository).expect("repo");
    fs::create_dir_all(&home).expect("home");
    fs::write(
        &fake_deadreckon,
        r#"#!/bin/sh
case "$1" in
  start) printf '%s\n' '{"dispatched":{"ids":["job-failed"]}}' ;;
  status) printf '%s\n' '{"job":{"projection":{"phase":"terminal","outcome":"retry_exhausted","stop_reason":"attempt_limit"}}}' ;;
  finish) exit 99 ;;
  *) exit 98 ;;
esac
"#,
    )
    .expect("fake deadreckon");
    let mut permissions = fs::metadata(&fake_deadreckon)
        .expect("fake metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_deadreckon, permissions).expect("fake executable");
    fs::write(
        &matrix,
        serde_json::to_vec_pretty(&json!({
            "repositories": [{
                "slot": "repo",
                "path_env": "WATCHKEEPER_TEST_REPO"
            }],
            "providers": [{
                "slot": "provider",
                "route_env": "WATCHKEEPER_TEST_PROVIDER"
            }],
            "tasks": [{
                "id": "failed-task",
                "repository": "repo",
                "provider": "provider",
                "goal": "observe a bounded failure",
                "max_spend_usd": 1.0
            }]
        }))
        .expect("matrix JSON"),
    )
    .expect("matrix");

    let output = Command::new("bash")
        .arg(dogfood_dir().join("run.sh"))
        .arg("failed-task")
        .env("DEADRECKON_DOGFOOD_EXECUTE", "1")
        .env("DEADRECKON_DOGFOOD_MATRIX", &matrix)
        .env("DEADRECKON_DOGFOOD_ARTIFACTS", &artifacts)
        .env("DEADRECKON_DOGFOOD_MAX_POLLS", "1")
        .env("DEADRECKON_HOME", &home)
        .env("DEADRECKON_BIN", &fake_deadreckon)
        .env("WATCHKEEPER_TEST_REPO", &repository)
        .env("WATCHKEEPER_TEST_PROVIDER", "smoke")
        .output()
        .expect("dogfood harness");

    assert!(!output.status.success(), "missing receipt must refuse");
    let observation_path = artifacts.join("failed-task/job-failed/operator-run.json");
    let observation: Value =
        serde_json::from_slice(&fs::read(observation_path).expect("operator observation"))
            .expect("operator observation JSON");
    assert_eq!(observation["terminal_outcome"], "retry_exhausted");
    assert_eq!(observation["terminal_stop_reason"], "attempt_limit");
    assert_eq!(observation["report_attempted"], true);
    assert_eq!(observation["report_succeeded"], false);
    assert_eq!(observation["receipt_validation_attempted"], true);
    assert_eq!(observation["receipt_validated"], false);
    assert_ne!(observation["receipt_validation_exit_status"], 0);
    assert_eq!(observation["finish_attempted"], false);
}

#[test]
fn metrics_are_derived_from_job_view_not_narrative() {
    let temp = TempDir::new().expect("tempdir");
    let home = temp.path().join("home");
    let observations = temp.path().join("observations/task-1/job-1");
    let job_dir = home.join("jobs/job-1");
    let proofs = home.join("runstate/scope-1/runs/job-1/proofs");
    let matrix = temp.path().join("matrix.json");
    fs::create_dir_all(&observations).expect("observations");
    fs::create_dir_all(&job_dir).expect("job dir");
    fs::create_dir_all(&proofs).expect("proofs");
    fs::write(
        &matrix,
        serde_json::to_vec_pretty(&json!({
            "repositories": [{
                "slot": "repo",
                "path_env": "WATCHKEEPER_TEST_REPO"
            }],
            "providers": [{
                "slot": "provider",
                "route_env": "WATCHKEEPER_TEST_PROVIDER"
            }],
            "tasks": [{
                "id": "task-1",
                "repository": "repo",
                "provider": "provider",
                "goal": "derive factual metrics",
                "max_spend_usd": 1.0
            }]
        }))
        .expect("matrix JSON"),
    )
    .expect("matrix");
    let matrix_sha256 = deadreckon_core::flight::sha256_file(&matrix).expect("matrix digest");

    let marker = proofs.join("turn-acceptance.json");
    fs::write(&marker, "{}\n").expect("marker");
    fs::write(
        proofs.join("semantic-judgment.json"),
        serde_json::to_vec_pretty(&json!({
            "job_id": "job-1",
            "run_id": "job-1",
            "spend_usd": 0.05
        }))
        .expect("judgment JSON"),
    )
    .expect("judgment");
    fs::write(
        observations.join("job-view.json"),
        serde_json::to_vec_pretty(&json!({
            "kind": "job_status",
            "job": {
                "job": { "job_id": "job-1" },
                "projection": {
                    "phase": "terminal",
                    "outcome": "verified",
                    "stop_reason": "verified"
                },
                "attempts": [{
                    "id": {
                        "scope": "scope-1",
                        "run_id": "job-1",
                        "short": "job-1"
                    },
                    "spend": {
                        "total_usd": 1.25,
                        "wall_seconds": 12.5
                    },
                    "proof": {
                        "marker_path": marker
                    }
                }]
            }
        }))
        .expect("JobView JSON"),
    )
    .expect("JobView");
    fs::write(
        job_dir.join("receipt.json"),
        serde_json::to_vec_pretty(&json!({
            "job_id": "job-1",
            "outcome": "verified",
            "proof_kind": "two_key_completion",
            "contained": true,
            "sandbox_backend": "sandbox-exec"
        }))
        .expect("receipt JSON"),
    )
    .expect("receipt");
    fs::write(
        observations.join("job-report.json"),
        serde_json::to_vec_pretty(&json!({
            "id": "job-1",
            "phase": "terminal",
            "outcome": "verified",
            "stop_reason": "verified",
            "receipt": {
                "status": "valid",
                "contained": true,
                "sandbox_backend": "sandbox-exec",
                "signature_validation_error": null,
                "receipt": {
                    "job_id": "job-1",
                    "proof_kind": "two_key_completion"
                }
            }
        }))
        .expect("report JSON"),
    )
    .expect("report");
    fs::write(
        observations.join("operator-run.json"),
        serde_json::to_vec_pretty(&json!({
            "task_id": "task-1",
            "job_id": "job-1",
            "matrix_sha256": matrix_sha256,
            "repository_slot": "repo",
            "provider_slot": "provider",
            "terminal_outcome": "verified",
            "terminal_stop_reason": "verified",
            "public_commands": ["start", "status", "report", "finish"],
            "report_attempted": true,
            "report_exit_status": 0,
            "report_succeeded": true,
            "receipt_validation_attempted": true,
            "receipt_validation_source": "deadreckon report --json",
            "receipt_validation_exit_status": 0,
            "receipt_validated": true,
            "finish_attempted": true,
            "finish_exit_status": 0,
            "finish_succeeded": true
        }))
        .expect("operator run JSON"),
    )
    .expect("operator run");
    let events = [
        json!({
            "timestamp": "2026-07-29T00:00:00Z",
            "kind": "created"
        }),
        json!({
            "timestamp": "2026-07-29T00:00:01Z",
            "kind": "lease_reclaimed"
        }),
        json!({
            "timestamp": "2026-07-29T00:00:02Z",
            "kind": "retry_scheduled"
        }),
        json!({
            "timestamp": "2026-07-29T00:00:03Z",
            "kind": "semantic_judge_revise"
        }),
        json!({
            "timestamp": "2026-07-29T00:00:10Z",
            "kind": "verified"
        }),
    ]
    .into_iter()
    .map(|event| serde_json::to_string(&event).expect("event JSON"))
    .collect::<Vec<_>>()
    .join("\n");
    fs::write(job_dir.join("job-events.jsonl"), format!("{events}\n")).expect("events");

    let narrative_sentinel = "NARRATIVE_SENTINEL_999999";
    fs::write(
        observations.join("narrative.md"),
        format!("Invented result: {narrative_sentinel}\n"),
    )
    .expect("narrative");
    let output_path = temp.path().join("metrics.json");
    let output = Command::new("python3")
        .arg(dogfood_dir().join("collect-metrics.py"))
        .arg("--home")
        .arg(&home)
        .arg("--observations")
        .arg(temp.path().join("observations"))
        .arg("--matrix")
        .arg(&matrix)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("Python 3 is required by the operator dogfood kit");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let raw = fs::read_to_string(output_path).expect("metrics output");
    let metrics: Value = serde_json::from_str(&raw).expect("metrics JSON");
    assert_eq!(metrics["jobs_observed"], 1);
    assert_eq!(metrics["persisted_facts"]["verified_jobs"], 1);
    assert_eq!(
        metrics["persisted_facts"]["unattended_verified_completion_rate"],
        1.0
    );
    assert_eq!(metrics["persisted_facts"]["automatic_recovery_rate"], 1.0);
    assert_eq!(metrics["persisted_facts"]["retry_count"], 1);
    assert_eq!(metrics["persisted_facts"]["semantic_revision_count"], 1);
    assert_eq!(metrics["persisted_facts"]["worker_spend_usd"], 1.25);
    assert_eq!(metrics["persisted_facts"]["judge_spend_usd"], 0.05);
    assert_eq!(metrics["data_quality"]["narrative_files_consulted"], 0);
    assert!(!raw.contains(narrative_sentinel));
}
