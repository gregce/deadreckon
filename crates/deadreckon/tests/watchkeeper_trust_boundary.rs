#![allow(clippy::expect_used)]

use std::fs;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use deadreckon_core::{DeadreckonPaths, JobView};
use deadreckon_protocol::{JobEventKind, JobOutcome};
use serde_json::Value;
use tempfile::TempDir;

#[test]
fn public_smoke_job_can_never_issue_a_trusted_completion_receipt() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");

    let launch = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .current_dir(&workspace)
        .env("DEADRECKON_HOME", paths.home())
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
    let ids = envelope["dispatched"]["ids"]
        .as_array()
        .expect("dispatched ids");
    assert_eq!(ids.len(), 1, "{envelope}");
    let job_id = ids[0].as_str().expect("job id");

    let deadline = Instant::now() + Duration::from_secs(20);
    let view = loop {
        if let Ok(view) = JobView::load(&paths, job_id)
            && view.projection.is_terminal()
        {
            break view;
        }
        assert!(
            Instant::now() < deadline,
            "public smoke Job {job_id} did not reach a bounded terminal state"
        );
        thread::sleep(Duration::from_millis(50));
    };

    assert_ne!(
        view.projection.outcome,
        Some(JobOutcome::Verified),
        "a scripted transport fixture cannot independently verify meaning"
    );
    assert!(
        !paths.job_receipt(job_id).exists(),
        "an untrusted smoke judgment must never produce a completion receipt"
    );
    let history =
        deadreckon_core::read_job_history(&paths.job_events(job_id)).expect("job history");
    assert!(
        history.events().iter().all(|event| {
            !matches!(
                event.kind,
                JobEventKind::SemanticJudgeAchieved | JobEventKind::Verified
            )
        }),
        "the append-only lifecycle must not claim semantic achievement or verification"
    );
}
