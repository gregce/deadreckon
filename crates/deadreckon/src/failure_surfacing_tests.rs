//! Failure-cause bubbling: plan- and campaign-level refusal surfaces must
//! name the underlying child failure reason (one hop away in state.json)
//! instead of only their own layer's status, and provider quota limits are
//! surfaced as resumable rather than generically failed.

use super::*;
use deadreckon_core::campaign::{Campaign, build_rollup, build_sub_goals};
use deadreckon_core::plan::{
    Plan, PlanMode, PlanProviders, PlanRole, PlanTask, PlanTaskStatus, save_plan,
};
use deadreckon_core::state::{RunOptions, RunStatus, create_run, save_state};
use tempfile::TempDir;

const QUOTA_REASON: &str = "provider error: CLI provider error for cli:claude-code: subprocess exited with Some(1): You've hit your session limit · resets 10:50pm (America/New_York)";

fn temp_paths() -> (TempDir, DeadreckonPaths) {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    (temp, paths)
}

fn failed_child_run(temp: &TempDir, paths: &DeadreckonPaths, reason: &str) -> String {
    let mut state = create_run(
        paths,
        RunOptions {
            goal: "child work".to_string(),
            cwd: temp.path().to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some("cli:claude-code".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");
    state.status = RunStatus::Failed;
    state.failure_reason = Some(reason.to_string());
    save_state(&state).expect("save state");
    state.run_id
}

fn plan_with_failed_child(child_run_id: Option<String>) -> Plan {
    let mut failed = PlanTask::new(
        0,
        "Task 0".to_string(),
        "Do child 0".to_string(),
        PlanRole::Child,
        Some("cli:claude-code".to_string()),
    );
    failed.status = PlanTaskStatus::Failed;
    failed.child_run_id = child_run_id;
    let pending = PlanTask::new(
        1,
        "Task 1".to_string(),
        "Do child 1".to_string(),
        PlanRole::Child,
        Some("cli:claude-code".to_string()),
    );
    Plan::new(
        "build the app",
        PlanMode::FullPlan,
        vec![failed, pending],
        PlanProviders::default(),
        Some("scope".to_string()),
        "0.1.0",
    )
    .expect("plan")
}

#[test]
fn merge_paused_surface_names_child_failure_reason_and_quota_resume() {
    let (temp, paths) = temp_paths();
    let run_id = failed_child_run(&temp, &paths, QUOTA_REASON);
    let plan = plan_with_failed_child(Some(run_id.clone()));

    let rendered = commands::merge::merge_incomplete_plan_surface(&paths, &plan, &plan.tasks[0])
        .render_plain(false);

    assert!(rendered.contains("session limit"), "{rendered}");
    let child = run_prefix(&run_id);
    assert!(
        rendered.contains(&format!("deadreckon show {child} --why-failed")),
        "{rendered}"
    );
    assert!(
        rendered.contains(&format!("deadreckon resume {child}")),
        "a quota-limited child is resumable, not dead: {rendered}"
    );
    assert!(rendered.contains("resets 10:50pm"), "{rendered}");
}

#[test]
fn merge_paused_surface_without_child_run_keeps_attach_primary() {
    let (_temp, paths) = temp_paths();
    let plan = plan_with_failed_child(None);

    let rendered = commands::merge::merge_incomplete_plan_surface(&paths, &plan, &plan.tasks[0])
        .render_plain(false);

    assert!(
        rendered.contains(&format!("deadreckon attach {}", run_prefix(&plan.plan_id))),
        "{rendered}"
    );
    assert!(!rendered.contains("reason"), "{rendered}");
}

#[test]
fn campaign_why_failed_names_per_sub_child_failure_reason() {
    let (temp, paths) = temp_paths();
    let run_id = failed_child_run(&temp, &paths, QUOTA_REASON);
    let plan = plan_with_failed_child(Some(run_id));
    save_plan(&paths, &plan).expect("save plan");

    let subs = build_sub_goals(
        vec!["rebuild billing".to_string(), "rebuild docs".to_string()],
        2,
    )
    .expect("subs");
    let mut campaign = Campaign::new(
        "rebuild everything",
        subs,
        PlanProviders::default(),
        0,
        Some(10.0),
        None,
        "0.1.0",
    )
    .expect("campaign");
    // Mirror the real failure shape: a refused sub never gets sub_plan_id
    // persisted; only its launch sidecar names the plan it ran.
    let launch_dir = paths
        .home()
        .join("plans")
        .join(&campaign.campaign_id)
        .join("launch")
        .join(&campaign.sub_goals[0].sub_id);
    std::fs::create_dir_all(&launch_dir).expect("launch dir");
    deadreckon_core::campaign::write_sub_result(
        &launch_dir,
        &deadreckon_core::campaign::SubResult {
            schema_version: 1,
            sub_id: campaign.sub_goals[0].sub_id.clone(),
            plan_id: Some(plan.plan_id.clone()),
            result_run_id: None,
            ok: false,
        },
    )
    .expect("sub result");
    campaign.sub_goals[1].result_run_id = Some("run-1".to_string());
    let rollup = build_rollup(&campaign, |_run_id| {
        (
            "signed".to_string(),
            deadreckon_core::tamper::AcceptanceTamperVerdict::Clean,
            Vec::new(),
        )
    });

    let report = commands::campaign::campaign_why_failed_report(&paths, &campaign, Some(&rollup));

    assert!(report.contains("session limit"), "{report}");
    assert!(report.contains(&campaign.sub_goals[0].sub_id), "{report}");
}

#[test]
fn provider_quota_note_extracts_reset_phrase() {
    let note = provider_quota_note(QUOTA_REASON).expect("quota recognized");
    assert!(note.contains("resets 10:50pm"), "{note}");

    assert!(provider_quota_note("provider error: connection refused").is_none());
}

#[test]
fn doctor_hint_suppressed_when_error_already_carries_try_line() {
    let with_try = CliError::Core(deadreckon_core::user_error(
        "campaign failed: refused sub(s) sub-0",
        "deadreckon show 65a65820 --why-failed",
    ));
    assert_eq!(error_hint(&with_try), "");

    let without_try = CliError::Core(deadreckon_core::error::DeadreckonError::InvalidInput(
        "something unmatched went wrong".to_string(),
    ));
    assert!(error_hint(&without_try).contains("doctor"));
}
