#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
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
    let receipt = source
        .find("jobs/$job_id/receipt.json")
        .expect("durable receipt");
    let finish = source
        .find("\"$deadreckon_bin\" finish")
        .expect("public finish");

    assert!(start < status);
    assert!(status < receipt);
    assert!(receipt < finish);
    assert!(source.contains("DEADRECKON_DOGFOOD_EXECUTE"));
    assert!(source.contains("\"contained\": True"));
    assert!(source.contains("\"proof_kind\": \"two_key_completion\""));
}

#[test]
fn dogfood_matrix_has_at_least_twenty_tasks_and_two_provider_slots() {
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
    assert!(
        tasks
            .iter()
            .all(|task| task["execution_status"] == "not_run")
    );
    assert_eq!(matrix["execution_policy"], "operator_only");
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
