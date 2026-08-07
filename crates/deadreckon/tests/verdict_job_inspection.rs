#![allow(clippy::expect_used)]
//! `verdict <job>` — read-only durable-Job inspection (G7 follow-up, APP-3).
//!
//! A durable Job reference maps onto the job's current attempt run
//! (`projection.json` `child_run_ids`, newest last; a Single-shape job's
//! attempt run IS the job id) and DEFAULTS to the receipt-style audit without
//! re-running checks: the attempt's run root belongs to the job's driver, so
//! inspection must leave it byte-identical — no sidecar, no re-execution.
//! `--rerun-checks` opts back into the normal re-run path, which still passes
//! through the driver fence and therefore keeps its typed public refusal.

use std::fs;
use std::process::{Command, Output};

use chrono::Utc;
use deadreckon_core::plan::{Plan, PlanMode, PlanProviders, PlanRole, PlanTask, save_plan};
use deadreckon_core::{
    DeadreckonPaths, RunOptions, RunOwnership, RunStatus, append_job_event, create_owned_run,
    save_state, write_job,
};
use deadreckon_protocol::{
    Job, JobEvent, JobEventKind, JobEventSequence, JobId, JobPolicy, JobSchemaVersion, JobShape,
    SemanticJudgeMode,
};
use serde_json::{Value, json};
use tempfile::TempDir;

fn deadreckon(paths: &DeadreckonPaths, workspace: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command
        .current_dir(workspace)
        .env("DEADRECKON_HOME", paths.home());
    command
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON ({error}):\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn directory_names(path: &std::path::Path) -> Vec<String> {
    let mut names = fs::read_dir(path)
        .expect("directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn write_job_fixture(paths: &DeadreckonPaths, workspace: &std::path::Path, job_id: &str) {
    write_job(
        paths,
        &Job {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(job_id.to_string()),
            scope: "watchkeeper-test".to_string(),
            goal: "job-owned inspection fixture".to_string(),
            shape: JobShape::Graph,
            created_at: Utc::now(),
            source_cwd: workspace.to_path_buf(),
            launch_plan_sha256: "fixture-launch-plan".to_string(),
            authority_sha256: "fixture-authority".to_string(),
            policy: JobPolicy {
                max_spend_usd: 1.0,
                max_wall_seconds: 60,
                max_attempts: 1,
                deadline: None,
                semantic_judge: SemanticJudgeMode::Required,
                execution: None,
            },
        },
    )
    .expect("write Job");
}

fn append_lifecycle_event(
    paths: &DeadreckonPaths,
    job_id: &str,
    sequence: u64,
    kind: JobEventKind,
    detail: Value,
) {
    append_job_event(
        paths,
        &JobEvent {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(job_id.to_string()),
            sequence: JobEventSequence::new(sequence).expect("event sequence"),
            event_id: format!("verdict-inspection-fixture-{sequence}"),
            causation_id: format!("verdict-inspection-fixture-{sequence}"),
            timestamp: Utc::now(),
            lease_epoch: 0,
            kind,
            detail,
        },
    )
    .expect("job lifecycle event");
}

/// A Graph-shape durable Job with one plan-task-owned attempt run linked into
/// its lifecycle history — the exact shape the driver fence guards.
fn fenced_graph_fixture(
    temp: &TempDir,
) -> (DeadreckonPaths, std::path::PathBuf, &'static str, String) {
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");
    let job_id = "76767676767676767676767676767676";
    write_job_fixture(&paths, &workspace, job_id);

    let mut plan = Plan::new(
        "complete one owned task",
        PlanMode::FullPlan,
        vec![
            PlanTask::new(
                0,
                "owned task",
                "complete owned task",
                PlanRole::Child,
                None,
            ),
            PlanTask::new(
                1,
                "later task",
                "complete later task",
                PlanRole::Child,
                None,
            ),
        ],
        PlanProviders::default(),
        Some("watchkeeper-test".to_string()),
        "test",
    )
    .expect("plan");
    plan.plan_id = job_id.to_string();
    plan.owner_job_id = Some(job_id.to_string());
    plan.parent_cwd = Some(workspace.clone());
    let mut attempt = create_owned_run(
        &paths,
        RunOptions {
            goal: "owned current attempt".to_string(),
            cwd: workspace.clone(),
            sandbox: "auto".to_string(),
            provider: Some("smoke".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: Some(60.0),
            run_id: None,
            codebase: None,
        },
        RunOwnership::plan_task(job_id, job_id, "task-0", 0, 1),
    )
    .expect("owned attempt Run");
    attempt.status = RunStatus::Completed;
    save_state(&attempt).expect("save attempt Run");
    plan.tasks[0].child_run_id = Some(attempt.run_id.clone());
    save_plan(&paths, &plan).expect("save linked Plan");

    append_lifecycle_event(&paths, job_id, 1, JobEventKind::Created, Value::Null);
    append_lifecycle_event(&paths, job_id, 2, JobEventKind::Queued, Value::Null);
    append_lifecycle_event(
        &paths,
        job_id,
        3,
        JobEventKind::AttemptStarted,
        json!({ "attempt": 1, "shape": "graph" }),
    );
    append_lifecycle_event(
        &paths,
        job_id,
        4,
        JobEventKind::ChildLinked,
        json!({ "attempt": 1, "run_id": attempt.run_id }),
    );

    (paths, workspace, job_id, attempt.run_id)
}

#[test]
fn job_ref_rerun_checks_keeps_the_typed_driver_fence_refusal() {
    // `--rerun-checks` is the mutating/re-run path: it maps the Job onto its
    // current attempt run and then takes the NORMAL verdict path — including
    // `require_current_driver_for_job_owned_run` — so a public invocation gets
    // the same typed refusal a direct run reference gets, and writes nothing.
    let temp = TempDir::new().expect("tempdir");
    let (paths, workspace, job_id, attempt_run_id) = fenced_graph_fixture(&temp);
    let run_root = deadreckon_core::load_run(&paths, &attempt_run_id)
        .expect("attempt state")
        .run_root;
    let files_before = directory_names(&run_root);
    let plan_before = fs::read(paths.plan_json(job_id)).expect("Plan before");

    for reference in [job_id, attempt_run_id.as_str()] {
        let mut arguments = vec!["verdict", reference];
        if reference == job_id {
            // The run reference is fenced with or without the flag; the Job
            // reference reaches the fence only on the explicit re-run path.
            arguments.push("--rerun-checks");
        }
        let output = deadreckon(&paths, &workspace)
            .args(&arguments)
            .output()
            .expect("public verdict");
        assert!(
            !output.status.success(),
            "stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("belongs to durable Job"), "{stderr}");
        assert_eq!(
            directory_names(&run_root),
            files_before,
            "refused verdict wrote into the job-owned run root"
        );
        assert_eq!(
            fs::read(paths.plan_json(job_id)).expect("Plan after"),
            plan_before
        );
    }
}

#[test]
fn job_ref_defaults_to_read_only_receipt_audit_inside_the_fence() {
    // The default Job path is inspection-only, so it succeeds where the re-run
    // path refuses — and proves it wrote nothing into the driver-owned run
    // root (no verdict sidecar, byte-identical file listing).
    let temp = TempDir::new().expect("tempdir");
    let (paths, workspace, job_id, attempt_run_id) = fenced_graph_fixture(&temp);
    let run_root = deadreckon_core::load_run(&paths, &attempt_run_id)
        .expect("attempt state")
        .run_root;
    let files_before = directory_names(&run_root);

    let output = deadreckon(&paths, &workspace)
        .args(["verdict", job_id, "--receipt", "--json"])
        .output()
        .expect("job receipt audit");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = stdout_json(&output);
    assert_eq!(envelope["kind"], "verdict");
    assert_eq!(envelope["id"], attempt_run_id.as_str());
    assert_eq!(envelope["job_id"], job_id);
    assert_eq!(envelope["mode"], "receipt_audit");
    assert_eq!(envelope["checks_rerun"], false);
    assert_eq!(envelope["paths"]["cache"], Value::Null);
    // No receipt was ever sealed for this attempt, so the audit records the
    // failed receipt_document fact instead of refusing — inspection reports.
    let facts = envelope["receipt_audit"]["facts"]
        .as_array()
        .expect("facts array");
    assert_eq!(facts[0]["name"], "receipt_document");
    assert_eq!(facts[0]["pass"], false);
    assert_eq!(envelope["status"], "unverified");

    assert_eq!(
        directory_names(&run_root),
        files_before,
        "read-only job inspection wrote into the driver-owned run root"
    );
}

#[test]
fn job_ref_without_any_attempt_run_refuses_toward_status() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");
    let job_id = "78787878787878787878787878787878";
    write_job_fixture(&paths, &workspace, job_id);

    let output = deadreckon(&paths, &workspace)
        .args(["verdict", job_id])
        .output()
        .expect("verdict with no attempt");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("has no attempt run to verify yet"),
        "{stderr}"
    );
    assert!(stderr.contains("deadreckon status"), "{stderr}");
}

/// The G7 target case end to end: a terminal VERIFIED Single-shape Job whose
/// two-key completion receipt is sealed and whose lifecycle history binds it.
/// `verdict <job> --receipt --json` must return the per-digest receipt audit
/// facts — all passing — without re-running checks or writing a sidecar.
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod terminal_verified {
    use std::fs;

    use chrono::Utc;
    use deadreckon_core::{
        AcceptanceCheckResult, AcceptanceContainment, DeadreckonPaths, JobView, PhaseId,
        PhaseStatus, RunOptions, create_run, read_gate_key, sandbox_boundary_result_tree_sha256,
        save_state, seal_completion_receipt, seal_sandbox_boundary_observation,
        validate_completion_receipt, write_job,
        write_native_acceptance_marker_with_results_and_key,
    };
    use deadreckon_protocol::{
        AuthorityAcceptedBy, GoalCoverage, GoalCoverageStatus, Job, JobAuthority, JobEventKind,
        JobExecutionPolicy, JobId, JobOutcome, JobPolicy, JobSchemaVersion, JobShape, RunId,
        SandboxBoundaryObservation, SandboxBoundaryObservationIssuer, SemanticDecision,
        SemanticJudgeMode, SemanticJudgment,
    };
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use uuid::Uuid;

    use super::{append_lifecycle_event, deadreckon, stdout_json};

    fn verified_single_fixture(temp: &TempDir) -> (DeadreckonPaths, Job) {
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("fixture-proof.txt"), "approved fixture\n").expect("source proof");
        let job_id = JobId("9abcdef01234567899abcdef01234567".to_string());
        let goal = "finish the verdict job inspection fixture".to_string();

        let job_dir = paths.job_dir(job_id.as_ref());
        fs::create_dir_all(&job_dir).expect("job directory");
        let launch_path = paths.job_launch_plan(job_id.as_ref());
        fs::write(
            &launch_path,
            serde_json::to_vec_pretty(&json!({
                "schema": "deadreckon.launch-plan.test.v1",
                "goal": goal,
                "shape": "single"
            }))
            .expect("launch JSON"),
        )
        .expect("launch plan");
        let contract_path = job_dir.join("acceptance.yaml");
        fs::write(
            &contract_path,
            "name: verdict job inspection\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/fixture-proof.txt\"\n",
        )
        .expect("approved contract");

        let policy = JobPolicy {
            max_spend_usd: 1.0,
            max_wall_seconds: 60,
            max_attempts: 1,
            deadline: None,
            semantic_judge: SemanticJudgeMode::Required,
            execution: Some(JobExecutionPolicy::workspace_only("auto")),
        };
        let authority = JobAuthority {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job_id.clone(),
            run_id: RunId(job_id.as_ref().to_string()),
            approved_at: Utc::now(),
            accepted_by: AuthorityAcceptedBy::Operator,
            goal_sha256: deadreckon_core::flight::sha256_text(&goal),
            contract_sha256: deadreckon_core::flight::sha256_file(&contract_path)
                .expect("contract digest"),
            effective_policy_sha256: deadreckon_core::flight::sha256_text(
                &serde_json::to_string(&policy).expect("policy JSON"),
            ),
            launch_plan_sha256: deadreckon_core::flight::sha256_file(&launch_path)
                .expect("launch digest"),
            source_tree_sha256: deadreckon_core::flight::build_deliverable_file_index(&source)
                .expect("source index")
                .tree_hash(),
            source_revision: None,
            sandbox_requested: "auto".to_string(),
            semantic_judge_mode: SemanticJudgeMode::Required,
            gate_evaluator_sha256: None,
        };
        let authority_path = paths.job_authority(job_id.as_ref());
        fs::write(
            &authority_path,
            serde_json::to_vec_pretty(&authority).expect("authority JSON"),
        )
        .expect("authority");
        let job = Job {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job_id.clone(),
            scope: deadreckon_core::paths::workspace_scope(&source).expect("source scope"),
            goal,
            shape: JobShape::Single,
            created_at: Utc::now(),
            source_cwd: source,
            launch_plan_sha256: authority.launch_plan_sha256.clone(),
            authority_sha256: deadreckon_core::flight::sha256_file(&authority_path)
                .expect("authority digest"),
            policy,
        };
        write_job(&paths, &job).expect("job identity");

        let mut state = create_run(
            &paths,
            RunOptions {
                goal: job.goal.clone(),
                cwd: job.source_cwd.clone(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(job.policy.max_spend_usd),
                max_wall_seconds: Some(job.policy.max_wall_seconds as f64),
                run_id: Some(job.job_id.as_ref().to_string()),
                codebase: None,
            },
        )
        .expect("result run");
        fs::copy(
            job.source_cwd.join("fixture-proof.txt"),
            state.working_dir.join("fixture-proof.txt"),
        )
        .expect("result proof");
        fs::copy(
            &contract_path,
            deadreckon_core::acceptance_spec_path_for_run_root(&state.run_root),
        )
        .expect("run contract");
        state
            .set_phase_status(PhaseId(60), PhaseStatus::Completed)
            .expect("completed result");
        save_state(&state).expect("result state");

        let key = read_gate_key(&paths, job.job_id.as_ref()).expect("gate key");
        let backend = if cfg!(target_os = "macos") {
            "sandbox-exec"
        } else {
            "bwrap"
        };
        let marker = write_native_acceptance_marker_with_results_and_key(
            &state.run_root,
            state.run_id.clone(),
            state.working_dir.clone(),
            vec![AcceptanceCheckResult {
                kind: "file_exists".to_string(),
                passed: true,
                must_pass: true,
                detail: "fixture result exists".to_string(),
                command: None,
                cwd: None,
                duration_ms: None,
                stdout: None,
                stderr: None,
            }],
            &key,
            AcceptanceContainment::contained(backend),
        )
        .expect("native marker");
        let outer_launch_id = Uuid::new_v4().to_string();
        let observation = SandboxBoundaryObservation {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job.job_id.clone(),
            run_id: RunId(job.job_id.as_ref().to_string()),
            observed_at: Utc::now(),
            issuer: SandboxBoundaryObservationIssuer::DeadreckonController,
            probe_id: Uuid::new_v4().to_string(),
            attempt: 1,
            outer_launch_id: outer_launch_id.clone(),
            authority_sha256: deadreckon_core::flight::sha256_file(&authority_path)
                .expect("observation authority digest"),
            contract_sha256: deadreckon_core::flight::sha256_file(
                &deadreckon_core::acceptance_spec_path_for_run_root(&state.run_root),
            )
            .expect("observation contract digest"),
            result_tree_sha256: sandbox_boundary_result_tree_sha256(&state)
                .expect("observation result digest"),
            sandbox_requested: authority.sandbox_requested.clone(),
            sandbox_backend: backend.to_string(),
            contained: true,
            gate_key_read_denied: true,
            proof_write_denied: true,
            control_write_denied: true,
            operator_capture_read_denied: true,
            operator_capture_write_denied: true,
            signing_env_scrubbed: true,
            probe_sha256: deadreckon_core::flight::sha256_text(
                "verdict job inspection boundary probe",
            ),
            gate_evaluator_sha256: None,
            signature: String::new(),
        };
        seal_sandbox_boundary_observation(&paths, &state, &authority, &observation)
            .expect("sandbox boundary observation");
        let judgment = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job.job_id.clone(),
            run_id: RunId(job.job_id.as_ref().to_string()),
            judged_at: Utc::now(),
            provider: "independent-test-judge".to_string(),
            model: "test-model".to_string(),
            decision: SemanticDecision::Achieved,
            summary: "the persisted result satisfies the approved goal".to_string(),
            goal_coverage: vec![GoalCoverage {
                claim: "approved inspection goal".to_string(),
                status: GoalCoverageStatus::Met,
                evidence: vec!["fixture-proof.txt".to_string()],
            }],
            missing: Vec::new(),
            input_sha256: deadreckon_core::flight::sha256_text("verdict job inspection input"),
            spend_usd: 0.0,
        };
        deadreckon_runtime::persist_semantic_judgment(&state.run_root, &judgment)
            .expect("semantic judgment");
        seal_completion_receipt(&paths, &state, &authority, &marker, &judgment)
            .expect("completion receipt");
        let receipt =
            validate_completion_receipt(&paths, &state).expect("validated completion receipt");

        // Fold the lifecycle history a supervisor would have recorded, ending
        // in the terminal Verified fact bound to the sealed receipt.
        let job_id = job.job_id.as_ref();
        append_lifecycle_event(&paths, job_id, 1, JobEventKind::Created, Value::Null);
        append_lifecycle_event(&paths, job_id, 2, JobEventKind::Queued, Value::Null);
        append_lifecycle_event(
            &paths,
            job_id,
            3,
            JobEventKind::AttemptStarted,
            json!({ "attempt": 1, "shape": "single" }),
        );
        append_lifecycle_event(
            &paths,
            job_id,
            4,
            JobEventKind::ChildLinked,
            json!({ "attempt": 1, "run_id": job_id }),
        );
        append_lifecycle_event(
            &paths,
            job_id,
            5,
            JobEventKind::Verified,
            json!({
                "receipt_sha256": deadreckon_core::flight::sha256_file(
                    &paths.job_receipt(job_id)
                )
                .expect("receipt digest"),
                "attempt": receipt.attempt,
                "receipt_run_id": receipt.run_id.as_ref(),
                "outer_launch_id": receipt.outer_launch_id,
                "result_run_id": receipt.run_id.as_ref(),
            }),
        );
        (paths, job)
    }

    /// The Single-shape twin of
    /// `job_ref_rerun_checks_keeps_the_typed_driver_fence_refusal`: a
    /// Single-shape job's attempt run has no stamped ownership and no
    /// plan/campaign reference (its run_id IS the job_id), so the run-level
    /// fence alone cannot see the owner. `--rerun-checks` must still refuse
    /// publicly via the artifact-level fence and write nothing into the
    /// job-owned run root.
    #[test]
    fn single_shape_job_ref_rerun_checks_keeps_the_typed_driver_fence_refusal() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = verified_single_fixture(&temp);
        let job_id = job.job_id.as_ref();
        let state = deadreckon_core::load_run(&paths, job_id).expect("attempt state");
        let proofs = state.run_root.join("proofs");
        let proofs_before = super::directory_names(&proofs);

        let output = deadreckon(&paths, &job.source_cwd)
            .args(["verdict", job_id, "--rerun-checks"])
            .output()
            .expect("public rerun verdict");
        assert!(
            !output.status.success(),
            "stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("belongs to durable Job"), "{stderr}");
        assert_eq!(
            super::directory_names(&proofs),
            proofs_before,
            "refused rerun verdict wrote into the job-owned run root"
        );
    }

    #[test]
    fn verdict_job_receipt_json_returns_passing_facts_for_a_terminal_verified_job() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = verified_single_fixture(&temp);
        let job_id = job.job_id.as_ref();

        // Fixture sanity: the projection is terminal Verified and the sealed
        // receipt is bound to that lifecycle fact.
        let view = JobView::load(&paths, job_id).expect("job view");
        assert_eq!(view.projection.outcome, Some(JobOutcome::Verified));
        assert!(view.projection.is_terminal());
        assert_eq!(view.verified_receipt_error, None);

        let state = deadreckon_core::load_run(&paths, job_id).expect("attempt state");
        let proofs = state.run_root.join("proofs");
        let sidecars = |dir: &std::path::Path| {
            fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|entry| entry.file_name().to_string_lossy().starts_with("verdict-"))
                        .count()
                })
                .unwrap_or(0)
        };
        assert_eq!(sidecars(&proofs), 0);

        let output = deadreckon(&paths, &job.source_cwd)
            .args(["verdict", job_id, "--receipt", "--json"])
            .output()
            .expect("job receipt audit");
        assert!(
            output.status.success(),
            "stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let envelope = stdout_json(&output);
        assert_eq!(envelope["kind"], "verdict");
        assert_eq!(envelope["id"], job_id);
        assert_eq!(envelope["job_id"], job_id);
        assert_eq!(envelope["mode"], "receipt_audit");
        assert_eq!(envelope["checks_rerun"], false);
        assert_eq!(envelope["status"], "verified");
        assert_eq!(envelope["had_signed_marker"], true);
        assert_eq!(envelope["marker_valid"], true);
        // No checks were re-executed on this path.
        assert_eq!(envelope["checks"], json!([]));
        let facts = envelope["receipt_audit"]["facts"]
            .as_array()
            .expect("facts array");
        assert!(!facts.is_empty(), "{envelope}");
        for fact in facts {
            assert_eq!(
                fact["pass"], true,
                "receipt audit fact failed on a sealed receipt: {fact}"
            );
        }
        assert!(
            facts.iter().any(|fact| fact["name"] == "receipt_document"),
            "{facts:?}"
        );

        // The inspection wrote nothing into the run root: no verdict sidecar,
        // and the envelope says so (`paths.cache` is null).
        assert_eq!(envelope["paths"]["cache"], Value::Null);
        assert_eq!(sidecars(&proofs), 0);
    }
}
