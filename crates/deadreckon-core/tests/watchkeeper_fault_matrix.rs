#![allow(clippy::expect_used)]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use deadreckon_core::flight::{build_deliverable_file_index, sha256_file, sha256_text};
use deadreckon_core::{
    AcceptanceCheckResult, AcceptanceContainment, DeadreckonPaths, JobHistory, LeaseOwner,
    PipelineState, RunOptions, append_fenced_job_event, append_job_event, claim_job_lease,
    create_run, promote_completed_run, read_gate_key, read_job_history, reduce_job_history,
    seal_completion_receipt, validate_completion_receipt, write_job,
    write_native_acceptance_marker_with_results_and_key,
};
use deadreckon_protocol::{
    AuthorityAcceptedBy, GoalCoverage, GoalCoverageStatus, Job, JobAuthority, JobEvent,
    JobEventKind, JobEventSequence, JobId, JobPolicy, JobSchemaVersion, JobShape, RunId,
    SemanticDecision, SemanticJudgeMode, SemanticJudgment,
};
use serde::Serialize;
use serde_json::json;
use tempfile::TempDir;

fn at(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("timestamp")
        .with_timezone(&Utc)
}

fn owner(name: &str, pid: u32) -> LeaseOwner {
    LeaseOwner {
        owner_id: name.to_string(),
        boot_id: "boot-a".to_string(),
        pid,
        process_group: pid,
    }
}

fn event(job_id: &JobId, sequence: u64, event_id: &str, kind: JobEventKind) -> JobEvent {
    JobEvent {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: job_id.clone(),
        sequence: JobEventSequence::new(sequence).expect("non-zero sequence"),
        event_id: event_id.to_string(),
        causation_id: event_id.to_string(),
        timestamp: at("2026-07-29T00:00:00Z"),
        lease_epoch: 0,
        kind,
        detail: json!({}),
    }
}

fn write_json(path: &std::path::Path, value: &impl Serialize) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("json parent");
    }
    fs::write(path, serde_json::to_vec_pretty(value).expect("json")).expect("write json");
}

struct CompletionFixture {
    _temp: TempDir,
    paths: DeadreckonPaths,
    state: PipelineState,
    authority: JobAuthority,
    marker: deadreckon_core::AcceptanceMarker,
    judgment: SemanticJudgment,
}

fn completion_fixture() -> CompletionFixture {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let source = temp.path().join("source");
    fs::create_dir_all(&source).expect("source");
    let state = create_run(
        &paths,
        RunOptions {
            goal: "ship verified change".to_string(),
            cwd: source,
            sandbox: "sandbox-exec".to_string(),
            provider: Some("judge".to_string()),
            skill_name: "deadreckon".to_string(),
            max_spend_usd: Some(2.0),
            max_wall_seconds: Some(60.0),
            run_id: Some("fault-receipt".to_string()),
            codebase: None,
        },
    )
    .expect("run");
    fs::create_dir_all(&state.working_dir).expect("working");
    fs::write(state.working_dir.join("result.txt"), "verified\n").expect("result");

    let contract_path = deadreckon_core::acceptance_spec_path_for_run_root(&state.run_root);
    fs::write(
        &contract_path,
        "name: result\nchecks:\n  - file_exists: result.txt\n",
    )
    .expect("contract");
    fs::create_dir_all(paths.job_dir("fault-receipt")).expect("job dir");
    let launch_path = paths.job_launch_plan("fault-receipt");
    fs::write(
        &launch_path,
        "{\"schema\":1,\"goal\":\"ship verified change\"}\n",
    )
    .expect("launch");

    let policy = JobPolicy {
        max_spend_usd: 2.0,
        max_wall_seconds: 60,
        max_attempts: 3,
        deadline: None,
        semantic_judge: SemanticJudgeMode::Required,
        execution: Some(deadreckon_protocol::JobExecutionPolicy::workspace_only(
            "sandbox-exec",
        )),
    };
    let authority = JobAuthority {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: JobId("fault-receipt".to_string()),
        run_id: RunId("fault-receipt".to_string()),
        approved_at: Utc::now(),
        accepted_by: AuthorityAcceptedBy::Operator,
        goal_sha256: sha256_text("ship verified change"),
        contract_sha256: sha256_file(&contract_path).expect("contract digest"),
        effective_policy_sha256: sha256_text(&serde_json::to_string(&policy).expect("policy json")),
        launch_plan_sha256: sha256_file(&launch_path).expect("launch digest"),
        source_tree_sha256: build_deliverable_file_index(&state.working_dir)
            .expect("source index")
            .tree_hash(),
        source_revision: None,
        sandbox_requested: "sandbox-exec".to_string(),
        semantic_judge_mode: SemanticJudgeMode::Required,
    };
    write_json(&paths.job_authority("fault-receipt"), &authority);
    let job = Job {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: JobId("fault-receipt".to_string()),
        scope: state.scope.clone(),
        goal: state.goal.clone(),
        shape: JobShape::Single,
        created_at: Utc::now(),
        source_cwd: state.cwd.clone(),
        launch_plan_sha256: authority.launch_plan_sha256.clone(),
        authority_sha256: sha256_file(&paths.job_authority("fault-receipt"))
            .expect("authority digest"),
        policy,
    };
    write_job(&paths, &job).expect("job");

    let key = read_gate_key(&paths, "fault-receipt").expect("key");
    let marker = write_native_acceptance_marker_with_results_and_key(
        &state.run_root,
        "fault-receipt".to_string(),
        state.working_dir.clone(),
        vec![AcceptanceCheckResult {
            kind: "file_exists".to_string(),
            passed: true,
            must_pass: true,
            detail: "result exists".to_string(),
            command: None,
            cwd: None,
            duration_ms: None,
            stdout: None,
            stderr: None,
        }],
        &key,
        AcceptanceContainment::contained("sandbox-exec"),
    )
    .expect("marker");
    let judgment = SemanticJudgment {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: JobId("fault-receipt".to_string()),
        run_id: RunId("fault-receipt".to_string()),
        judged_at: Utc::now(),
        provider: "judge".to_string(),
        model: "judge-model".to_string(),
        decision: SemanticDecision::Achieved,
        summary: "the result satisfies the approved goal".to_string(),
        goal_coverage: vec![GoalCoverage {
            claim: "ship verified change".to_string(),
            status: GoalCoverageStatus::Met,
            evidence: vec!["deterministic-gate".to_string()],
        }],
        missing: Vec::new(),
        input_sha256: sha256_text("evidence"),
        spend_usd: 0.01,
    };
    write_json(
        &state.run_root.join(deadreckon_core::SEMANTIC_JUDGMENT_JSON),
        &judgment,
    );

    CompletionFixture {
        _temp: temp,
        paths,
        state,
        authority,
        marker,
        judgment,
    }
}

fn assert_torn_append_fails_closed() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path());
    let job_id = JobId("fault-torn-event".to_string());
    append_job_event(&paths, &event(&job_id, 1, "created", JobEventKind::Created))
        .expect("first event");

    let events_path = paths.job_events(job_id.as_ref());
    let mut events = OpenOptions::new()
        .append(true)
        .open(&events_path)
        .expect("event log");
    events
        .write_all(b"{\"schema_version\":1,\"job_id\":")
        .expect("torn append");
    events.sync_all().expect("sync torn append");

    let history: JobHistory = read_job_history(&events_path).expect("recover complete rows");
    assert_eq!(history.events().len(), 1);
    assert_eq!(history.caveats.len(), 1);
    assert!(history.caveats[0].contains("torn final job event"));
    assert_eq!(
        reduce_job_history(&job_id, &history)
            .expect("reduce intact prefix")
            .last_sequence,
        1
    );

    let error = append_job_event(&paths, &event(&job_id, 2, "queued", JobEventKind::Queued))
        .expect_err("must not append over a torn row");
    assert!(
        error
            .to_string()
            .contains("cannot append after a torn final event row")
    );
}

fn assert_lease_reclaim_and_fencing() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path());
    let job_id = JobId("fault-lease".to_string());
    let now = at("2026-07-29T00:00:00Z");
    let stale = claim_job_lease(
        &paths,
        &job_id,
        &owner("supervisor-a", 4100),
        now,
        Duration::from_secs(15),
    )
    .expect("first claim")
    .token();

    let live_claim = claim_job_lease(
        &paths,
        &job_id,
        &owner("supervisor-b", 4200),
        now + TimeDelta::seconds(1),
        Duration::from_secs(15),
    )
    .expect_err("live lease cannot be stolen");
    assert!(live_claim.to_string().contains("live lease held"));

    let current = claim_job_lease(
        &paths,
        &job_id,
        &owner("supervisor-b", 4200),
        now + TimeDelta::seconds(15),
        Duration::from_secs(15),
    )
    .expect("expired lease reclaim");
    assert_eq!(current.lease.epoch, stale.epoch + 1);

    let stale_event = JobEvent {
        lease_epoch: stale.epoch,
        timestamp: now + TimeDelta::seconds(16),
        ..event(&job_id, 3, "stale-attempt", JobEventKind::AttemptStarted)
    };
    let error = append_fenced_job_event(&paths, &stale, now + TimeDelta::seconds(16), &stale_event)
        .expect_err("old epoch cannot commit");
    assert!(error.to_string().contains("stale lease token"));
}

fn assert_receipt_tamper_blocks_validation_and_promotion() {
    let mut fixture = completion_fixture();
    let mut receipt = seal_completion_receipt(
        &fixture.paths,
        &fixture.state,
        &fixture.authority,
        &fixture.marker,
        &fixture.judgment,
    )
    .expect("seal receipt");
    validate_completion_receipt(&fixture.paths, &fixture.state).expect("validate sealed receipt");

    receipt.signature = "00".repeat(32);
    write_json(
        &fixture.paths.job_receipt(fixture.authority.job_id.as_ref()),
        &receipt,
    );
    let validation_error = validate_completion_receipt(&fixture.paths, &fixture.state)
        .expect_err("tampered HMAC must fail");
    assert!(
        validation_error
            .to_string()
            .contains("receipt signature verification failed")
    );

    let original_working_dir = fixture.state.working_dir.clone();
    let library_dir = fixture
        .paths
        .library_dir(&fixture.state.scope, &fixture.state.run_id);
    let promotion_error = promote_completed_run(&fixture.paths, &mut fixture.state)
        .expect_err("promotion must refuse an invalid receipt");
    assert!(
        promotion_error
            .to_string()
            .contains("receipt signature verification failed")
    );
    assert_eq!(fixture.state.working_dir, original_working_dir);
    assert!(!library_dir.exists());
}

#[test]
fn fault_matrix_covers_every_durable_boundary() {
    // This deliberately bounded matrix names every durable boundary in scope:
    // event history, ownership/fencing, authenticated completion, promotion.
    let mut covered = Vec::new();

    assert_torn_append_fails_closed();
    covered.push("append/event torn write");

    assert_lease_reclaim_and_fencing();
    covered.push("lease claim/reclaim/fencing");

    assert_receipt_tamper_blocks_validation_and_promotion();
    covered.push("receipt seal/validation/tamper");
    covered.push("promotion refusal");

    assert_eq!(
        covered,
        [
            "append/event torn write",
            "lease claim/reclaim/fencing",
            "receipt seal/validation/tamper",
            "promotion refusal",
        ]
    );
}

#[test]
fn two_supervisors_racing_execute_each_job_once() {
    // Core cannot execute provider work. This proves the narrower durable
    // invariant: of two racing supervisors, only the lease winner can commit
    // the AttemptStarted admission event for this job.
    let temp = TempDir::new().expect("tempdir");
    let paths = Arc::new(DeadreckonPaths::from_home(temp.path()));
    let barrier = Arc::new(Barrier::new(3));
    let job_id = JobId("fault-racing-supervisors".to_string());
    let now = at("2026-07-29T00:00:00Z");
    let mut workers = Vec::new();

    for index in 1..=2 {
        let paths = Arc::clone(&paths);
        let barrier = Arc::clone(&barrier);
        let job_id = job_id.clone();
        workers.push(thread::spawn(move || {
            barrier.wait();
            let claim = claim_job_lease(
                &paths,
                &job_id,
                &owner(&format!("supervisor-{index}"), 5000 + index),
                now,
                Duration::from_secs(15),
            )?;
            let token = claim.token();
            let attempt = JobEvent {
                lease_epoch: token.epoch,
                ..event(
                    &job_id,
                    2,
                    &format!("attempt-started-{index}"),
                    JobEventKind::AttemptStarted,
                )
            };
            append_fenced_job_event(&paths, &token, now, &attempt)?;
            Ok::<_, deadreckon_core::DeadreckonError>(token)
        }));
    }
    barrier.wait();
    let results = workers
        .into_iter()
        .map(|worker| worker.join().expect("worker"))
        .collect::<Vec<_>>();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    let winner = results
        .iter()
        .find_map(|result| result.as_ref().ok())
        .expect("one winner");
    assert_eq!(winner.epoch, 1);

    let history =
        read_job_history(&paths.job_events(job_id.as_ref())).expect("durable history after race");
    assert_eq!(
        history
            .events()
            .iter()
            .filter(|event| event.kind == JobEventKind::LeaseAcquired)
            .count(),
        1
    );
    assert_eq!(
        history
            .events()
            .iter()
            .filter(|event| event.kind == JobEventKind::AttemptStarted)
            .count(),
        1
    );
    let projection = reduce_job_history(&job_id, &history).expect("projection");
    assert_eq!(projection.current_lease_epoch, 1);
    assert_eq!(projection.attempt_count, 1);
}
