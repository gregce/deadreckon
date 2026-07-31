#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
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
fn adversarial_runner_names_each_boundary_and_keeps_live_claims_unproven() {
    let source =
        fs::read_to_string(dogfood_dir().join("adversarial.py")).expect("adversarial runner");
    for trial in [
        "terminal_detach",
        "worker_kill",
        "supervisor_restart",
        "network_denial",
        "gate_key_search_and_forgery",
        "docker_control_boundary",
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
    for unified_job_proof in [
        "guided_continuation_preserves_approved_authority_provenance",
        "durable_chain_freezes_one_graph_job_with_at_end_verification",
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
    assert!(source.contains("\"status\": \"unproven\""));
    assert!(source.contains("\"matrix_status\": matrix_status(repo)"));
    assert!(source.contains("\"seatbelt_preflight\": seatbelt"));
    assert!(source.contains("\"docker_preflight\": docker"));
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
        "docker_gate_boundary",
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
        "docker_gate_boundary",
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
    validate_credential_free_source(
        &payload,
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    )
    .unwrap_or_else(|error| panic!("{error}"));
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
        "docker_control_boundary",
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
