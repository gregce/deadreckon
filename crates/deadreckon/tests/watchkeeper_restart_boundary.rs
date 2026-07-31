#![allow(clippy::expect_used)]

use std::fs;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use deadreckon_core::{DeadreckonPaths, JobView, load_job_lease, pid_is_alive, read_job_history};
use deadreckon_protocol::{JobEventKind, JobId, JobOutcome};
use serde_json::Value;
use tempfile::TempDir;

mod common;

use common::SupervisorServiceFixture;

const FAILPOINT_ENABLE_ENV: &str = "DEADRECKON_TEST_SUPERVISOR_FAILPOINTS";
const FAILPOINT_ENV: &str = "DEADRECKON_TEST_SUPERVISOR_FAILPOINT";

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn every_pre_release_crash_relaunches_the_same_attempt_without_mutating_early() {
    for failpoint in [
        "after_launch_prepared",
        "after_attempt_started",
        "after_guarded_spawn",
        "after_child_metadata",
        "after_child_linked",
    ] {
        assert_pre_release_crash_recovers(failpoint);
    }
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn crash_after_private_release_adopts_or_recovers_without_duplicating_the_attempt() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");
    let service = SupervisorServiceFixture::configured_with_env(
        &paths,
        &[
            ("DEADRECKON_BOOT_ID", "watchkeeper-before-release-crash"),
            (FAILPOINT_ENABLE_ENV, "1"),
            (FAILPOINT_ENV, "after_child_released"),
        ],
    );

    let launch = service
        .deadreckon()
        .current_dir(&workspace)
        .args([
            "start",
            "Create the deterministic smoke project and satisfy its checks.",
            "--mode",
            "run",
            "--provider",
            "smoke",
            "--fresh",
            "--max-spend",
            "1",
            "--yes",
            "--plain",
            "--json",
        ])
        .output()
        .expect("public start");
    assert!(
        launch.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&launch.stdout),
        String::from_utf8_lossy(&launch.stderr)
    );
    let envelope: Value = serde_json::from_slice(&launch.stdout).expect("launch JSON");
    let job_id = envelope["dispatched"]["ids"][0].as_str().expect("job id");
    wait_for_event(&paths, job_id, JobEventKind::ChildLinked);
    wait_for_failed_supervisor_exit(&paths, job_id);

    let recovery = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .current_dir(&workspace)
        .env("DEADRECKON_HOME", paths.home())
        .env("DEADRECKON_BOOT_ID", "watchkeeper-after-release-crash")
        .args(["supervisor", "serve", "--once", job_id])
        .output()
        .expect("recovery supervisor");
    assert!(
        recovery.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&recovery.stdout),
        String::from_utf8_lossy(&recovery.stderr)
    );

    let view = JobView::load(&paths, job_id).expect("recovered Job view");
    assert!(view.projection.is_terminal(), "{view:?}");
    assert_eq!(
        view.projection.attempt_count, 1,
        "a supervisor crash after release must not duplicate the logical attempt"
    );
    assert_ne!(view.projection.outcome, Some(JobOutcome::Verified));
    let history = read_job_history(&paths.job_events(job_id)).expect("job history");
    assert_eq!(
        history
            .events()
            .iter()
            .filter(|event| event.kind == JobEventKind::AttemptStarted)
            .count(),
        1
    );
    assert!(
        history.events().iter().all(|event| {
            event.detail.get("stop_reason").and_then(Value::as_str) != Some("lost_containment")
        }),
        "the acknowledged release has enough identity to recover without lost containment"
    );
    assert!(
        !paths.job_dir(job_id).join("supervised-child.json").exists(),
        "recovery left stale child metadata"
    );
    assert!(
        fs::read_dir(paths.job_dir(job_id))
            .expect("job directory")
            .flatten()
            .all(|entry| {
                !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("supervised-release-")
            }),
        "recovery left a stale private release acknowledgement"
    );
}

fn assert_pre_release_crash_recovers(failpoint: &str) {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");
    let service = SupervisorServiceFixture::configured_with_env(
        &paths,
        &[
            ("DEADRECKON_BOOT_ID", "watchkeeper-before-crash"),
            (FAILPOINT_ENABLE_ENV, "1"),
            (FAILPOINT_ENV, failpoint),
        ],
    );

    let launch = service
        .deadreckon()
        .current_dir(&workspace)
        .args([
            "start",
            "Create the deterministic smoke project and satisfy its checks.",
            "--mode",
            "run",
            "--provider",
            "smoke",
            "--fresh",
            "--max-spend",
            "1",
            "--yes",
            "--plain",
            "--json",
        ])
        .output()
        .expect("public start");
    assert!(
        launch.status.success(),
        "{failpoint}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&launch.stdout),
        String::from_utf8_lossy(&launch.stderr)
    );
    let envelope: Value = serde_json::from_slice(&launch.stdout).expect("launch JSON");
    let job_id = envelope["dispatched"]["ids"][0].as_str().expect("job id");

    wait_for_event(&paths, job_id, expected_last_event(failpoint));
    wait_for_failed_supervisor_exit(&paths, job_id);
    assert!(
        deadreckon_core::load_run(&paths, job_id).is_err(),
        "{failpoint} allowed a worker to persist run state before durable release"
    );
    let acknowledged = fs::read_dir(paths.job_dir(job_id))
        .expect("job control directory")
        .flatten()
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("supervised-release-")
        });
    assert!(
        !acknowledged,
        "{failpoint} acknowledged release before the parent sent the private token"
    );

    let recovery = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .current_dir(&workspace)
        .env("DEADRECKON_HOME", paths.home())
        .env("DEADRECKON_BOOT_ID", "watchkeeper-after-crash")
        .args(["supervisor", "serve", "--once", job_id])
        .output()
        .expect("recovery supervisor");
    assert!(
        recovery.status.success(),
        "{failpoint}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&recovery.stdout),
        String::from_utf8_lossy(&recovery.stderr)
    );

    let view = JobView::load(&paths, job_id).expect("recovered Job view");
    assert!(view.projection.is_terminal(), "{failpoint}: {view:?}");
    assert_eq!(
        view.projection.attempt_count, 1,
        "{failpoint} consumed a retry even though no worker had been released"
    );
    assert_ne!(
        view.projection.outcome,
        Some(JobOutcome::Verified),
        "scripted smoke still cannot supply the semantic completion key"
    );
    let history = read_job_history(&paths.job_events(job_id)).expect("job history");
    let attempts = history
        .events()
        .iter()
        .filter(|event| event.kind == JobEventKind::AttemptStarted)
        .count();
    let prepared = history
        .events()
        .iter()
        .filter(|event| event.kind == JobEventKind::ChildLaunchPrepared)
        .count();
    let linked = history
        .events()
        .iter()
        .filter(|event| event.kind == JobEventKind::ChildLinked)
        .count();
    assert_eq!(attempts, 1, "{failpoint}");
    assert_eq!(prepared, 2, "{failpoint}");
    assert!(linked >= 1, "{failpoint}");
    let first_prepared_sequence = history
        .events()
        .iter()
        .find(|event| event.kind == JobEventKind::ChildLaunchPrepared)
        .expect("prepared sequence")
        .sequence;
    let attempt_sequence = history
        .events()
        .iter()
        .find(|event| event.kind == JobEventKind::AttemptStarted)
        .expect("attempt sequence")
        .sequence;
    let final_link_sequence = history
        .events()
        .iter()
        .rev()
        .find(|event| event.kind == JobEventKind::ChildLinked)
        .expect("linked sequence")
        .sequence;
    assert!(
        first_prepared_sequence < attempt_sequence && attempt_sequence < final_link_sequence,
        "{failpoint} did not preserve prepare -> attempt -> link ordering"
    );
    assert!(
        history.events().iter().all(|event| {
            event.detail.get("stop_reason").and_then(Value::as_str) != Some("lost_containment")
        }),
        "{failpoint} incorrectly became lost containment"
    );
    assert!(
        !paths.job_dir(job_id).join("supervised-child.json").exists(),
        "{failpoint} left stale child metadata"
    );
}

fn wait_for_failed_supervisor_exit(paths: &DeadreckonPaths, job_id: &str) {
    let lease = load_job_lease(paths, &JobId(job_id.to_string())).expect("failed supervisor lease");
    let deadline = Instant::now() + Duration::from_secs(10);
    while pid_is_alive(lease.pid) {
        assert!(
            Instant::now() < deadline,
            "failpoint supervisor {} did not exit",
            lease.pid
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn expected_last_event(failpoint: &str) -> JobEventKind {
    match failpoint {
        "after_launch_prepared" => JobEventKind::ChildLaunchPrepared,
        "after_attempt_started" | "after_guarded_spawn" | "after_child_metadata" => {
            JobEventKind::AttemptStarted
        }
        "after_child_linked" => JobEventKind::ChildLinked,
        _ => panic!("unknown failpoint {failpoint}"),
    }
}

fn wait_for_event(paths: &DeadreckonPaths, job_id: &str, kind: JobEventKind) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(history) = read_job_history(&paths.job_events(job_id))
            && history.events().iter().any(|event| event.kind == kind)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "job {job_id} never reached failpoint event {kind:?}"
        );
        thread::sleep(Duration::from_millis(25));
    }
}
