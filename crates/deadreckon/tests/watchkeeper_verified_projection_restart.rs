#![allow(clippy::expect_used)]

use std::fs;
use std::process::Command;
use std::time::Duration;

use chrono::Utc;
use deadreckon_core::{
    AcceptanceCheckResult, AcceptanceContainment, DeadreckonPaths, JobView, LeaseOwner, PhaseId,
    PhaseStatus, RunOptions, append_fenced_job_event, append_job_event, claim_job_lease,
    create_run, load_job_lease, read_gate_key, read_job_history,
    sandbox_boundary_result_tree_sha256, save_state, seal_completion_receipt,
    seal_sandbox_boundary_observation, validate_completion_receipt, write_job,
    write_native_acceptance_marker_with_results_and_key,
};
use deadreckon_protocol::{
    AuthorityAcceptedBy, GoalCoverage, GoalCoverageStatus, Job, JobAuthority, JobEvent,
    JobEventKind, JobEventSequence, JobExecutionPolicy, JobId, JobOutcome, JobPolicy,
    JobSchemaVersion, JobShape, RunId, SandboxBoundaryObservation,
    SandboxBoundaryObservationIssuer, SemanticDecision, SemanticJudgeMode, SemanticJudgment,
    StopReason,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use uuid::Uuid;

const FAILPOINT_ENABLE_ENV: &str = "DEADRECKON_TEST_SUPERVISOR_FAILPOINTS";
const FAILPOINT_ENV: &str = "DEADRECKON_TEST_SUPERVISOR_FAILPOINT";
const BOOT_ID: &str = "watchkeeper-verified-projection-restart";

struct VerifiedProjectionBoundary {
    failpoint: &'static str,
    last_partial_event: JobEventKind,
    gate_events: usize,
    semantic_events: usize,
    stopped_events: usize,
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn verified_projection_failpoints_recover_exactly_once_without_relaunch() {
    for boundary in [
        VerifiedProjectionBoundary {
            failpoint: "after_verified_receipt_before_control_events",
            last_partial_event: JobEventKind::LeaseReclaimed,
            gate_events: 0,
            semantic_events: 0,
            stopped_events: 0,
        },
        VerifiedProjectionBoundary {
            failpoint: "after_semantic_achieved_before_attempt_stopped",
            last_partial_event: JobEventKind::SemanticJudgeAchieved,
            gate_events: 1,
            semantic_events: 1,
            stopped_events: 0,
        },
        VerifiedProjectionBoundary {
            failpoint: "after_verified_attempt_stopped_before_terminal",
            last_partial_event: JobEventKind::AttemptStopped,
            gate_events: 1,
            semantic_events: 1,
            stopped_events: 1,
        },
    ] {
        assert_verified_projection_boundary(&boundary);
    }
}

fn assert_verified_projection_boundary(boundary: &VerifiedProjectionBoundary) {
    let temp = TempDir::new().expect("tempdir");
    let (paths, job) = verified_single_fixture(&temp);

    let crash = supervisor_command(&paths, &job)
        .env(FAILPOINT_ENABLE_ENV, "1")
        .env(FAILPOINT_ENV, boundary.failpoint)
        .output()
        .expect("run failpoint supervisor");
    let post_process_history =
        read_job_history(&paths.job_events(job.job_id.as_ref())).expect("post-process Job history");
    assert_eq!(
        crash.status.code(),
        Some(86),
        "{}\nstdout:\n{}\nstderr:\n{}\nhistory:\n{:?}",
        boundary.failpoint,
        String::from_utf8_lossy(&crash.stdout),
        String::from_utf8_lossy(&crash.stderr),
        post_process_history.events()
    );

    let partial = post_process_history;
    assert_eq!(
        partial.events().last().map(|event| event.kind),
        Some(boundary.last_partial_event),
        "{} stopped at the wrong durable boundary: {:?}",
        boundary.failpoint,
        partial.events()
    );
    assert_eq!(
        event_count(&partial, JobEventKind::DeterministicGatePassed),
        boundary.gate_events,
        "{} deterministic projection",
        boundary.failpoint
    );
    assert_eq!(
        event_count(&partial, JobEventKind::SemanticJudgeAchieved),
        boundary.semantic_events,
        "{} semantic projection",
        boundary.failpoint
    );
    assert_eq!(
        event_count(&partial, JobEventKind::AttemptStopped),
        boundary.stopped_events,
        "{} attempt-stop projection",
        boundary.failpoint
    );
    assert_eq!(event_count(&partial, JobEventKind::Verified), 0);
    assert_eq!(event_count(&partial, JobEventKind::AttemptStarted), 1);
    assert_eq!(
        event_count(&partial, JobEventKind::ChildLaunchPrepared),
        1,
        "{} prepared another worker before crashing",
        boundary.failpoint
    );
    assert_eq!(event_count(&partial, JobEventKind::ChildLinked), 1);

    expire_lease(&paths, &job);
    let recovery = supervisor_command(&paths, &job)
        .output()
        .expect("run replacement supervisor");
    assert!(
        recovery.status.success(),
        "{}\nstdout:\n{}\nstderr:\n{}",
        boundary.failpoint,
        String::from_utf8_lossy(&recovery.stdout),
        String::from_utf8_lossy(&recovery.stderr)
    );

    let view = JobView::load(&paths, job.job_id.as_ref()).expect("recovered Job view");
    assert_eq!(view.projection.outcome, Some(JobOutcome::Verified));
    assert_eq!(view.projection.stop_reason, Some(StopReason::Verified));
    assert_eq!(view.projection.attempt_count, 1);
    let recovered =
        read_job_history(&paths.job_events(job.job_id.as_ref())).expect("recovered Job history");
    for kind in [
        JobEventKind::AttemptStarted,
        JobEventKind::DeterministicGatePassed,
        JobEventKind::SemanticJudgeAchieved,
        JobEventKind::AttemptStopped,
        JobEventKind::Verified,
    ] {
        assert_eq!(
            event_count(&recovered, kind),
            1,
            "{} duplicated {kind:?}: {:?}",
            boundary.failpoint,
            recovered.events()
        );
    }
    assert_eq!(
        event_count(&recovered, JobEventKind::LeaseReclaimed),
        2,
        "{} did not reclaim both expired supervisor leases",
        boundary.failpoint
    );
    assert_eq!(
        event_count(&recovered, JobEventKind::ChildLaunchPrepared),
        1,
        "{} prepared another provider launch: {:?}",
        boundary.failpoint,
        recovered.events()
    );
    assert_eq!(event_count(&recovered, JobEventKind::ChildLinked), 1);
    assert_eq!(event_count(&recovered, JobEventKind::RetryScheduled), 0);
}

fn supervisor_command(paths: &DeadreckonPaths, job: &Job) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command
        .current_dir(&job.source_cwd)
        .env("DEADRECKON_HOME", paths.home())
        .env("DEADRECKON_BOOT_ID", BOOT_ID)
        .env_remove(FAILPOINT_ENABLE_ENV)
        .env_remove(FAILPOINT_ENV)
        .args(["supervisor", "serve", "--once", job.job_id.as_ref()]);
    command
}

fn verified_single_fixture(temp: &TempDir) -> (DeadreckonPaths, Job) {
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let source = temp.path().join("source");
    fs::create_dir_all(&source).expect("source");
    fs::write(source.join("fixture-proof.txt"), "approved fixture\n").expect("source proof");
    let job_id = JobId("1234567890abcdef1234567890abcdef".to_string());
    let goal = "finish the verified projection restart fixture".to_string();

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
        "name: verified projection restart\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/fixture-proof.txt\"\n",
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
    for (index, kind) in [JobEventKind::Created, JobEventKind::Queued]
        .into_iter()
        .enumerate()
    {
        append_job_event(
            &paths,
            &JobEvent {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: job_id.clone(),
                sequence: JobEventSequence::new(index as u64 + 1).expect("event sequence"),
                event_id: format!("verified-projection-fixture-{index}"),
                causation_id: format!("verified-projection-fixture-{index}"),
                timestamp: Utc::now(),
                lease_epoch: 0,
                kind,
                detail: Value::Null,
            },
        )
        .expect("initial Job event");
    }

    let setup_owner = LeaseOwner {
        owner_id: "verified-projection-fixture".to_string(),
        boot_id: BOOT_ID.to_string(),
        pid: std::process::id(),
        process_group: std::process::id(),
    };
    let setup_claim = claim_job_lease(
        &paths,
        &job.job_id,
        &setup_owner,
        Utc::now(),
        Duration::from_secs(30),
    )
    .expect("setup lease");
    let setup_token = setup_claim.token();
    let outer_launch_id = Uuid::new_v4().to_string();
    let release_token_sha256 = deadreckon_core::flight::sha256_text("fixture-release-token");
    for (event_id, kind, detail) in [
        (
            "verified-projection-launch-prepared",
            JobEventKind::ChildLaunchPrepared,
            json!({
                "attempt": 1,
                "launch_id": outer_launch_id,
                "launch_protocol": "stdin_release_v1",
                "release_token_sha256": release_token_sha256,
                "shape": "single"
            }),
        ),
        (
            "verified-projection-attempt",
            JobEventKind::AttemptStarted,
            json!({ "attempt": 1, "shape": "single" }),
        ),
        (
            "verified-projection-child-linked",
            JobEventKind::ChildLinked,
            json!({
                "attempt": 1,
                "launch_id": outer_launch_id,
                "launch_protocol": "stdin_release_v1",
                "release_token_sha256": release_token_sha256,
                "run_id": job.job_id.as_ref(),
                "adopted": false
            }),
        ),
    ] {
        let projection = JobView::load(&paths, job.job_id.as_ref())
            .expect("leased Job view")
            .projection;
        append_fenced_job_event(
            &paths,
            &setup_token,
            Utc::now(),
            &JobEvent {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: job.job_id.clone(),
                sequence: JobEventSequence::new(projection.last_sequence + 1)
                    .expect("setup event sequence"),
                event_id: event_id.to_string(),
                causation_id: event_id.to_string(),
                timestamp: Utc::now(),
                lease_epoch: setup_token.epoch,
                kind,
                detail,
            },
        )
        .expect("setup lifecycle event");
    }

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
    let observation = SandboxBoundaryObservation {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: job.job_id.clone(),
        run_id: RunId(job.job_id.as_ref().to_string()),
        observed_at: Utc::now(),
        issuer: SandboxBoundaryObservationIssuer::DeadreckonController,
        probe_id: Uuid::new_v4().to_string(),
        attempt: 1,
        outer_launch_id,
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
            "verified projection restart boundary probe",
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
            claim: "approved restart goal".to_string(),
            status: GoalCoverageStatus::Met,
            evidence: vec!["fixture-proof.txt".to_string()],
        }],
        missing: Vec::new(),
        input_sha256: deadreckon_core::flight::sha256_text(
            "verified projection restart semantic input",
        ),
        spend_usd: 0.0,
    };
    deadreckon_runtime::persist_semantic_judgment(&state.run_root, &judgment)
        .expect("semantic judgment");
    seal_completion_receipt(&paths, &state, &authority, &marker, &judgment)
        .expect("completion receipt");
    validate_completion_receipt(&paths, &state).expect("validated completion receipt");
    expire_lease(&paths, &job);
    (paths, job)
}

fn expire_lease(paths: &DeadreckonPaths, job: &Job) {
    let mut lease = load_job_lease(paths, &job.job_id).expect("active supervisor lease");
    lease.expires_at = Utc::now() - chrono::Duration::seconds(1);
    // Model an owner that actually exited. Expiry alone must not let another
    // supervisor steal authority from a still-live same-boot process now that
    // leases bind the process-start identity.
    lease.process_start_identity = Some("exited-fixture-owner".to_string());
    fs::write(
        paths.job_lease(job.job_id.as_ref()),
        serde_json::to_vec_pretty(&lease).expect("expired lease JSON"),
    )
    .expect("expire supervisor lease");
}

fn event_count(history: &deadreckon_core::JobHistory, kind: JobEventKind) -> usize {
    history
        .events()
        .iter()
        .filter(|event| event.kind == kind)
        .count()
}
