#![allow(clippy::expect_used)]
//! `show <job> --diff` — the G10 changes surface through a durable Job ref
//! (APP-3 follow-up).
//!
//! For a Single-shape durable Job the attempt's run id IS the job id, so the
//! resolver hands `show <job>` the Job kind. Diff-shaped requests
//! (`--diff [--patch --file]`, `--turn`) must not be silently dropped into
//! the job-status envelope: they resolve the job's current attempt run — the
//! same rule `verdict <job>` uses — and answer with the run's diff surfaces.
//! Plain `show <job>` stays byte-identical with `status <job>` (both render
//! `print_job_status`).

use std::fs;
use std::process::{Command, Output};

use chrono::Utc;
use deadreckon_core::{
    DeadreckonPaths, RunOptions, RunStatus, append_job_event, create_run, save_state,
    snapshot_working, write_job,
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

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
            event_id: format!("show-job-diff-fixture-{sequence}"),
            causation_id: format!("show-job-diff-fixture-{sequence}"),
            timestamp: Utc::now(),
            lease_epoch: 0,
            kind,
            detail,
        },
    )
    .expect("job lifecycle event");
}

/// A Single-shape durable Job whose one attempt run (run id = job id) carries
/// turn-0 and turn-1 snapshots — the minimal shape the diff surface needs.
fn single_shape_fixture(temp: &TempDir) -> (DeadreckonPaths, std::path::PathBuf, &'static str) {
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");
    let job_id = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
    write_job(
        &paths,
        &Job {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(job_id.to_string()),
            scope: "watchkeeper-test".to_string(),
            goal: "single-shape diff fixture".to_string(),
            shape: JobShape::Single,
            created_at: Utc::now(),
            source_cwd: workspace.clone(),
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

    let mut state = create_run(
        &paths,
        RunOptions {
            goal: "single-shape diff fixture".to_string(),
            cwd: workspace.clone(),
            sandbox: "none".to_string(),
            provider: Some("smoke".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: Some(60.0),
            run_id: Some(job_id.to_string()),
            codebase: None,
        },
    )
    .expect("attempt run");
    fs::write(state.working_dir.join("README.md"), "before\n").expect("readme before");
    snapshot_working(&state, 0).expect("snapshot 0");
    fs::write(state.working_dir.join("README.md"), "before\nafter\n").expect("readme after");
    state.turn = 1;
    state.status = RunStatus::Completed;
    save_state(&state).expect("save attempt run");
    snapshot_working(&state, 1).expect("snapshot 1");

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
    // A terminal classification keeps the job-status envelope deterministic:
    // a live job renders a wall-clock allowance that drifts between the two
    // invocations the byte-compat pin compares.
    append_lifecycle_event(
        &paths,
        job_id,
        5,
        JobEventKind::Failed,
        json!({ "reason": "fixture terminal classification" }),
    );

    (paths, workspace, job_id)
}

#[test]
fn show_job_diff_json_returns_the_diff_summary_not_job_status() {
    // The registered gap: the Job branch used to route straight to
    // `print_job_status`, so `--diff --json` answered with a job_status
    // envelope where the app asked for a DiffSummary.
    let temp = TempDir::new().expect("tempdir");
    let (paths, workspace, job_id) = single_shape_fixture(&temp);

    let output = deadreckon(&paths, &workspace)
        .args(["show", job_id, "--diff", "--json", "--plain"])
        .output()
        .expect("show job diff");
    assert_success(&output);
    let value = stdout_json(&output);
    assert_eq!(
        value.get("kind"),
        None,
        "job_status leaked through: {value}"
    );
    assert_eq!(value["files_changed"], 1, "{value}");
    let files = value["files"].as_array().expect("files array");
    assert_eq!(files.len(), 1, "{value}");
    assert_eq!(files[0]["path"], "README.md", "{value}");
}

#[test]
fn show_job_diff_patch_file_exports_the_full_patch_through_the_job_ref() {
    let temp = TempDir::new().expect("tempdir");
    let (paths, workspace, job_id) = single_shape_fixture(&temp);

    let output = deadreckon(&paths, &workspace)
        .args([
            "show",
            job_id,
            "--diff",
            "--patch",
            "--file",
            "README.md",
            "--json",
            "--plain",
        ])
        .output()
        .expect("show job diff patch file");
    assert_success(&output);
    let value = stdout_json(&output);
    let patches = value["patches"].as_array().expect("patches array");
    assert_eq!(patches.len(), 1, "{value}");
    assert_eq!(patches[0]["path"], "README.md", "{value}");
    assert_eq!(patches[0]["truncated"], false, "{value}");
    let unified = patches[0]["unified"].as_str().expect("unified text");
    assert!(unified.contains("+after"), "{unified}");
}

#[test]
fn plain_show_job_still_renders_the_job_status_envelope() {
    // Byte-compat pin: without diff-shaped flags the Job branch keeps its
    // exact historical routing — `print_job_status`, the same renderer
    // `status <job>` uses — so `show <job>` and `status <job>` stay
    // byte-identical in both prose and JSON.
    let temp = TempDir::new().expect("tempdir");
    let (paths, workspace, job_id) = single_shape_fixture(&temp);

    for json in [false, true] {
        let mut show_args = vec!["show", job_id, "--plain"];
        let mut status_args = vec!["status", job_id, "--plain"];
        if json {
            show_args.push("--json");
            status_args.push("--json");
        }
        let show = deadreckon(&paths, &workspace)
            .args(&show_args)
            .output()
            .expect("show job");
        let status = deadreckon(&paths, &workspace)
            .args(&status_args)
            .output()
            .expect("status job");
        assert_success(&show);
        assert_success(&status);
        assert_eq!(
            show.stdout,
            status.stdout,
            "show <job> diverged from status <job> (json={json}):\nshow:\n{}\nstatus:\n{}",
            String::from_utf8_lossy(&show.stdout),
            String::from_utf8_lossy(&status.stdout)
        );
        if json {
            let value = stdout_json(&show);
            assert_eq!(value["kind"], "job_status", "{value}");
            assert_eq!(value["id"], job_id, "{value}");
        }
    }
}

#[test]
fn show_job_turn_answers_with_the_attempt_runs_turn_view() {
    // `--turn` rides the same delegation: the Job ref maps onto the current
    // attempt run and answers with that run's per-turn view.
    let temp = TempDir::new().expect("tempdir");
    let (paths, workspace, job_id) = single_shape_fixture(&temp);

    let output = deadreckon(&paths, &workspace)
        .args(["show", job_id, "--turn", "1", "--json", "--plain"])
        .output()
        .expect("show job turn");
    assert_success(&output);
    let value = stdout_json(&output);
    assert_eq!(value["n"], 1, "{value}");
    assert_eq!(value["diff"]["files"][0]["path"], "README.md", "{value}");
}
