#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};
use tempfile::TempDir;

fn dogfood_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/watchkeeper-dogfood")
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
fn adversarial_runner_names_each_boundary_and_keeps_live_claims_unproven() {
    let source =
        fs::read_to_string(dogfood_dir().join("adversarial.py")).expect("adversarial runner");
    for trial in [
        "terminal_detach",
        "worker_kill",
        "supervisor_restart",
        "network_denial",
        "gate_key_search_and_forgery",
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
        "docker_gate_boundary",
    ] {
        assert!(
            source.contains(unproven),
            "missing live boundary {unproven}"
        );
    }
    assert!(source.contains("\"status\": \"unproven\""));
    assert!(source.contains("\"matrix_status\": matrix_status(repo)"));
    assert!(source.contains("\"seatbelt_preflight\": seatbelt"));
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
    assert_eq!(payload["matrix_status"]["total_tasks"], tasks.len());
    for (status, count) in expected_counts {
        assert_eq!(
            payload["matrix_status"]["by_execution_status"][status],
            count
        );
    }
    assert_eq!(payload["summary"]["failed"], 0);
    assert_eq!(
        payload["runner_sha256"],
        deadreckon_core::flight::sha256_file(&dogfood_dir().join("adversarial.py"))
            .expect("runner digest")
    );
    let trials = payload["trials"]
        .as_array()
        .expect("credential-free trials");
    let expected_trials = [
        "terminal_detach",
        "worker_kill",
        "supervisor_restart",
        "network_denial",
        "gate_key_search_and_forgery",
        "receipt_mutation",
        "result_delivery",
        "unified_job_journey",
        "job_child_and_planner_boundaries",
        "semantic_parent_repair",
        "repair_lineage_tamper",
    ];
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
    let live = payload["live_claims"].as_array().expect("live claims");
    assert_eq!(live.len(), 9);
    assert!(
        live.iter()
            .all(|claim| claim["status"].as_str() == Some("unproven"))
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
    let proofs = temp.path().join("run/proofs");
    fs::create_dir_all(&observations).expect("observations");
    fs::create_dir_all(&job_dir).expect("job dir");
    fs::create_dir_all(&proofs).expect("proofs");

    let marker = proofs.join("turn-acceptance.json");
    fs::write(&marker, "{}\n").expect("marker");
    fs::write(
        proofs.join("semantic-judgment.json"),
        serde_json::to_vec_pretty(&json!({ "spend_usd": 0.05 })).expect("judgment JSON"),
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
            "job_id": "job-1",
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
