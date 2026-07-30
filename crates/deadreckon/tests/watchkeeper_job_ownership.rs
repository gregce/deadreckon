#![allow(clippy::expect_used)]

#[cfg(target_os = "macos")]
use std::collections::BTreeSet;
use std::fs;
use std::process::{Command, Output};
#[cfg(target_os = "macos")]
use std::thread;
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

use chrono::Utc;
use deadreckon_core::campaign::{Campaign, CampaignStatus, build_sub_goals, write_campaign};
use deadreckon_core::plan::{
    Plan, PlanMode, PlanProviders, PlanRole, PlanStatus, PlanTask, PlanTaskStatus, TaskAttempt,
    save_plan,
};
use deadreckon_core::{
    DeadreckonPaths, RunOptions, RunOwnership, RunStatus, create_owned_run, save_state, write_job,
};
#[cfg(target_os = "macos")]
use deadreckon_core::{JobView, read_job_history, read_plan_events};
#[cfg(target_os = "macos")]
use deadreckon_protocol::JobOutcome;
use deadreckon_protocol::{Job, JobId, JobPolicy, JobSchemaVersion, JobShape, SemanticJudgeMode};
#[cfg(target_os = "macos")]
use serde_json::Value;
use tempfile::TempDir;

#[test]
fn public_fork_and_merge_cannot_mutate_a_job_owned_plan() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");
    let job_id = "11111111111111111111111111111111";
    write_job_fixture(&paths, &workspace, job_id, JobShape::Graph);

    let tasks = vec![
        PlanTask::new(0, "first", "complete first", PlanRole::Child, None),
        PlanTask::new(1, "second", "complete second", PlanRole::Child, None),
    ];
    let mut plan = Plan::new(
        "complete both",
        PlanMode::FullPlan,
        tasks,
        PlanProviders::default(),
        Some("watchkeeper-test".to_string()),
        "test",
    )
    .expect("plan");
    plan.plan_id = job_id.to_string();
    plan.owner_job_id = Some(job_id.to_string());
    plan.parent_cwd = Some(workspace.clone());
    let mut child = Plan::new(
        "complete nested work",
        PlanMode::FullPlan,
        vec![
            PlanTask::new(
                0,
                "nested first",
                "complete nested first",
                PlanRole::Child,
                None,
            ),
            PlanTask::new(
                1,
                "nested second",
                "complete nested second",
                PlanRole::Child,
                None,
            ),
        ],
        PlanProviders::default(),
        Some("watchkeeper-test".to_string()),
        "test",
    )
    .expect("nested plan");
    child.plan_id = "11111111111111111111111111111112".to_string();
    child.owner_job_id = Some(job_id.to_string());
    child.parent_plan_id = Some(job_id.to_string());
    child.parent_cwd = Some(workspace.clone());
    plan.tasks[0].subplan = Some(child.plan_id.clone());
    let mut nested_result = create_owned_run(
        &paths,
        RunOptions {
            goal: child.root_goal.clone(),
            cwd: workspace.clone(),
            sandbox: "auto".to_string(),
            provider: Some("smoke".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: Some(60.0),
            run_id: None,
            codebase: None,
        },
        RunOwnership::plan_result(job_id, &child.plan_id),
    )
    .expect("owned nested result");
    nested_result.status = RunStatus::Completed;
    save_state(&nested_result).expect("save nested result");
    child.merged_run_id = Some(nested_result.run_id.clone());
    child.status = PlanStatus::Merged;
    save_plan(&paths, &child).expect("save nested plan");
    save_plan(&paths, &plan).expect("save plan");
    let root_before = fs::read(paths.plan_json(job_id)).expect("root Plan before");
    let child_before = fs::read(paths.plan_json(&child.plan_id)).expect("child Plan before");

    for target in [job_id, child.plan_id.as_str()] {
        for arguments in [
            vec!["fork", target, "--yes", "--quiet"],
            vec!["merge", target, "--yes", "--quiet"],
        ] {
            let output = deadreckon(&paths, &workspace)
                .args(arguments)
                .output()
                .expect("public plan mutation");
            assert_job_owned_refusal(&output);
            assert_eq!(
                fs::read(paths.plan_json(job_id)).expect("root Plan after"),
                root_before
            );
            assert_eq!(
                fs::read(paths.plan_json(&child.plan_id)).expect("child Plan after"),
                child_before
            );
            assert!(
                !paths.plan_events(target).exists(),
                "public command wrote Plan events before refusing"
            );
        }
    }
    let export = temp.path().join("forbidden-plan-export");
    let output = deadreckon(&paths, &workspace)
        .args([
            "finish",
            &child.plan_id,
            "--dest",
            export.to_str().expect("export path"),
        ])
        .output()
        .expect("public nested Plan finish");
    assert_job_owned_refusal(&output);
    assert!(
        !export.exists(),
        "public nested Plan finish exported a result"
    );
    assert_eq!(
        fs::read_dir(paths.jobs_dir())
            .expect("jobs")
            .flatten()
            .count(),
        1,
        "public fork compiled a second durable Job"
    );
}

#[test]
fn public_resume_and_extend_cannot_mutate_a_job_owned_child() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");
    let job_id = "12121212121212121212121212121212";
    write_job_fixture(&paths, &workspace, job_id, JobShape::Graph);

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
    plan.tasks[0].status = PlanTaskStatus::Running;
    save_plan(&paths, &plan).expect("save awaiting Plan");

    let mut first = create_owned_run(
        &paths,
        RunOptions {
            goal: "first owned attempt".to_string(),
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
    .expect("owned first Run");
    first.status = RunStatus::Completed;
    save_state(&first).expect("save first Run");

    let first_before = fs::read(first.state_path()).expect("first state before");
    let output = deadreckon(&paths, &workspace)
        .args(["resume", &first.run_id])
        .output()
        .expect("public resume in ownership creation window");
    assert_job_owned_refusal(&output);
    assert_eq!(
        fs::read(first.state_path()).expect("first state after"),
        first_before
    );

    plan.tasks[0].attempts.push(TaskAttempt::failed(
        1,
        Some(first.run_id.clone()),
        Some("retry fixture".to_string()),
        0.0,
    ));
    let mut current = create_owned_run(
        &paths,
        RunOptions {
            goal: "current owned attempt".to_string(),
            cwd: workspace.clone(),
            sandbox: "auto".to_string(),
            provider: Some("smoke".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: Some(60.0),
            run_id: None,
            codebase: None,
        },
        RunOwnership::plan_task(job_id, job_id, "task-0", 0, 2),
    )
    .expect("owned current Run");
    current.status = RunStatus::Completed;
    save_state(&current).expect("save current Run");
    plan.tasks[0].child_run_id = Some(current.run_id.clone());
    save_plan(&paths, &plan).expect("save linked Plan");

    let plan_before = fs::read(paths.plan_json(job_id)).expect("Plan before");
    let current_before = fs::read(current.state_path()).expect("current state before");
    for arguments in [
        vec!["resume".to_string(), format!("{job_id}:task-0")],
        vec![
            "extend".to_string(),
            current.run_id.clone(),
            "unauthorized follow-up".to_string(),
        ],
        vec!["resume".to_string(), first.run_id.clone()],
        vec!["finish".to_string(), current.run_id.clone()],
        vec!["apply".to_string(), current.run_id.clone()],
        vec!["abandon".to_string(), current.run_id.clone()],
        vec!["cleanup".to_string(), current.run_id.clone()],
        vec!["undo".to_string(), current.run_id.clone()],
        vec![
            "rewind".to_string(),
            current.run_id.clone(),
            "--preview".to_string(),
        ],
        vec![
            "steer".to_string(),
            current.run_id.clone(),
            "change the approved task".to_string(),
        ],
        vec![
            "doc".to_string(),
            current.run_id.clone(),
            "--polish".to_string(),
            "--no-confirm".to_string(),
            "--force".to_string(),
        ],
    ] {
        let output = deadreckon(&paths, &workspace)
            .args(arguments)
            .output()
            .expect("public child mutation");
        assert_job_owned_refusal(&output);
        assert_eq!(
            fs::read(paths.plan_json(job_id)).expect("Plan after"),
            plan_before
        );
        assert_eq!(
            fs::read(first.state_path()).expect("first state after"),
            first_before
        );
        assert_eq!(
            fs::read(current.state_path()).expect("current state after"),
            current_before
        );
    }

    let output = deadreckon(&paths, &workspace)
        .args(["kill", &current.run_id])
        .output()
        .expect("kill owned child");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        deadreckon_core::read_job_history(&paths.job_events(job_id))
            .expect("Job history")
            .events()
            .iter()
            .any(|event| matches!(
                event.kind,
                deadreckon_protocol::JobEventKind::CancelRequested
            )),
        "killing a child did not cancel its durable Job"
    );
    assert_eq!(
        fs::read(current.state_path()).expect("current state after Job cancellation"),
        current_before,
        "Job cancellation mutated the child Run directly"
    );
}

#[test]
fn public_campaign_repair_cannot_mutate_a_job_owned_campaign() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");
    let job_id = "22222222222222222222222222222222";
    write_job_fixture(&paths, &workspace, job_id, JobShape::LegacyCampaign);

    let mut campaign = Campaign::new(
        "complete both campaign branches",
        build_sub_goals(
            vec!["complete first".to_string(), "complete second".to_string()],
            2,
        )
        .expect("sub-goals"),
        PlanProviders::default(),
        0,
        None,
        None,
        "test",
    )
    .expect("campaign");
    campaign.campaign_id = job_id.to_string();
    campaign.status = CampaignStatus::Failed;
    let campaign_dir = paths.plan_dir(job_id);
    fs::create_dir_all(&campaign_dir).expect("campaign dir");
    write_campaign(&campaign_dir, &campaign).expect("write campaign");
    let before = fs::read(campaign_dir.join("campaign.json")).expect("campaign before");
    let files_before = directory_names(&campaign_dir);

    let output = deadreckon(&paths, &workspace)
        .args(["campaign", "repair", job_id, "--quiet", "--plain"])
        .output()
        .expect("public campaign repair");

    assert_job_owned_refusal(&output);
    assert_eq!(
        fs::read(campaign_dir.join("campaign.json")).expect("campaign after"),
        before
    );
    assert_eq!(
        directory_names(&campaign_dir),
        files_before,
        "public campaign repair wrote evidence before refusing"
    );
}

#[test]
fn public_campaign_result_cannot_bypass_its_job_lifecycle() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");
    let job_id = "23232323232323232323232323232323";
    write_job_fixture(&paths, &workspace, job_id, JobShape::LegacyCampaign);

    let mut campaign = Campaign::new(
        "complete both campaign branches",
        build_sub_goals(
            vec!["complete first".to_string(), "complete second".to_string()],
            2,
        )
        .expect("sub-goals"),
        PlanProviders::default(),
        0,
        None,
        None,
        "test",
    )
    .expect("campaign");
    campaign.campaign_id = job_id.to_string();
    campaign.status = CampaignStatus::Forked;
    let campaign_dir = paths.plan_dir(job_id);
    fs::create_dir_all(&campaign_dir).expect("campaign dir");
    write_campaign(&campaign_dir, &campaign).expect("write campaign creation window");

    let mut result = create_owned_run(
        &paths,
        RunOptions {
            goal: campaign.root_goal.clone(),
            cwd: workspace.clone(),
            sandbox: "auto".to_string(),
            provider: Some("smoke".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: Some(60.0),
            run_id: None,
            codebase: None,
        },
        RunOwnership::campaign_result(job_id, job_id),
    )
    .expect("owned Campaign result");
    result.status = RunStatus::Completed;
    save_state(&result).expect("save Campaign result");

    let state_before = fs::read(result.state_path()).expect("Campaign result before");
    let output = deadreckon(&paths, &workspace)
        .args(["resume", &result.run_id])
        .output()
        .expect("public Campaign result resume during creation window");
    assert_job_owned_refusal(&output);
    assert_eq!(
        fs::read(result.state_path()).expect("Campaign result after"),
        state_before
    );

    campaign.merged_run_id = Some(result.run_id.clone());
    campaign.status = CampaignStatus::Merged;
    write_campaign(&campaign_dir, &campaign).expect("write linked Campaign");
    let output = deadreckon(&paths, &workspace)
        .args(["doc", &result.run_id, "--polish", "--no-confirm", "--force"])
        .output()
        .expect("public Campaign result doc polish");
    assert_job_owned_refusal(&output);
    assert_eq!(
        fs::read(result.state_path()).expect("Campaign result after doc polish"),
        state_before
    );
}

#[test]
fn job_id_environment_cannot_authorize_hidden_driver_entry_points() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");
    let job_id = "33333333333333333333333333333333";
    write_job_fixture(&paths, &workspace, job_id, JobShape::Graph);
    let files_before = directory_names(&paths.job_dir(job_id));

    for arguments in [
        vec!["supervisor", "drive", job_id],
        vec!["supervisor", "resume", job_id],
        vec![
            "run",
            "attempt to impersonate a trusted leaf driver",
            "--run-id",
            job_id,
            "--quiet",
            "--plain",
        ],
    ] {
        let output = deadreckon(&paths, &workspace)
            .env("DEADRECKON_SUPERVISOR_JOB_ID", job_id)
            .args(arguments)
            .output()
            .expect("spoofed driver command");
        assert!(
            !output.status.success(),
            "stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("guarded attempt identity"),
            "a Job ID environment value reached a trusted driver route:\n{stderr}"
        );
        assert_eq!(
            directory_names(&paths.job_dir(job_id)),
            files_before,
            "spoofed driver command mutated durable Job state"
        );
    }
}

#[test]
fn public_run_cannot_spoof_a_subordinate_job_child() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");
    let fake_parent = "44444444444444444444444444444444";

    let output = deadreckon(&paths, &workspace)
        .env("DEADRECKON_DELEGATION_JOB", fake_parent)
        .env(
            "DEADRECKON_DELEGATION_ID",
            "00000000-0000-4000-8000-000000000001",
        )
        .env("DEADRECKON_SCOPE_ROOT", &workspace)
        .args([
            "run",
            "attempt to impersonate a subordinate child",
            "--from",
            workspace.to_str().expect("workspace path"),
            "--yes",
            "--quiet",
            "--plain",
        ])
        .output()
        .expect("spoofed subordinate run");

    assert!(
        !output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("delegations") || stderr.contains("delegated invocation"),
        "{stderr}"
    );
    assert!(
        !paths.jobs_dir().exists(),
        "spoofed subordinate route created a root Job"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn trusted_supervisor_can_mutate_and_merge_its_job_owned_plan() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(workspace.join(".deadreckon")).expect("acceptance dir");
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(paths.config_path(), "default_provider = \"smoke\"\n").expect("config");
    fs::write(workspace.join("README.md"), "watchkeeper graph smoke\n").expect("readme");
    fs::write(
        workspace.join(".deadreckon/acceptance.yaml"),
        concat!(
            "name: trusted graph driver\n",
            "checks:\n",
            "  - kind: file_exists\n",
            "    path: \"{working_dir}/README.md\"\n",
        ),
    )
    .expect("acceptance");
    git(&workspace, &["init", "--initial-branch=main"]);
    git(
        &workspace,
        &["config", "user.email", "watchkeeper@example.invalid"],
    );
    git(&workspace, &["config", "user.name", "Watchkeeper Test"]);
    git(&workspace, &["add", "-A"]);
    git(&workspace, &["commit", "-m", "fixture"]);

    let output = deadreckon(&paths, &workspace)
        .args([
            "start",
            "Exercise the durable graph driver with two isolated smoke tasks.",
            "--mode",
            "full-plan",
            "--children",
            "2",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--max-spend",
            "1",
            "--yes",
            "--quiet",
            "--plain",
            "--json",
        ])
        .output()
        .expect("public graph start");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("start JSON");
    let job_id = envelope["dispatched"]["ids"][0]
        .as_str()
        .expect("dispatched Job ID");

    let deadline = Instant::now() + Duration::from_secs(60);
    let view = loop {
        if let Ok(view) = JobView::load(&paths, job_id)
            && view.projection.is_terminal()
        {
            break view;
        }
        assert!(
            Instant::now() < deadline,
            "Graph Job {job_id} did not reach a bounded terminal state\n{}",
            fs::read_to_string(paths.job_dir(job_id).join("supervisor-stderr.log"))
                .unwrap_or_default()
        );
        thread::sleep(Duration::from_millis(50));
    };

    let supervisor_stderr =
        fs::read_to_string(paths.job_dir(job_id).join("supervisor-stderr.log")).unwrap_or_default();
    let plan = deadreckon_core::load_plan(&paths, job_id).unwrap_or_else(|error| {
        let history = deadreckon_core::read_job_history(&paths.job_events(job_id))
            .map(|history| format!("{:#?}", history.events()))
            .unwrap_or_default();
        let driver_stderr =
            fs::read_to_string(paths.job_dir(job_id).join("supervisor.err")).unwrap_or_default();
        panic!(
            "Job-owned Plan: {error}\nJob:\n{view:#?}\nEvents:\n{history}\nSupervisor stderr:\n{supervisor_stderr}\nDriver stderr:\n{driver_stderr}"
        )
    });
    let plan_events = read_plan_events(&paths, job_id).expect("Plan events");
    assert_eq!(plan.plan_id, job_id);
    assert_eq!(
        plan.status,
        deadreckon_core::PlanStatus::Merged,
        "Plan:\n{plan:#?}\nEvents:\n{plan_events:#?}\nJob:\n{view:#?}\nSupervisor stderr:\n{supervisor_stderr}"
    );
    assert!(
        plan_events.iter().any(|event| matches!(
            event.event,
            deadreckon_core::PlanEventKind::MergeCompleted { .. }
        )),
        "trusted driver never completed the Job-owned Plan merge"
    );
    assert_eq!(
        fs::read_dir(paths.jobs_dir())
            .expect("jobs")
            .flatten()
            .count(),
        1,
        "trusted graph execution created another root Job"
    );
    assert_eq!(
        view.projection.outcome,
        Some(JobOutcome::NeedsReview),
        "the scripted judge must fail closed after the deterministic graph succeeds\nJob:\n{view:#?}\nEvents:\n{:#?}\nSupervisor stderr:\n{supervisor_stderr}\nDriver stderr:\n{}",
        read_job_history(&paths.job_events(job_id))
            .map(|history| history.events().to_vec())
            .unwrap_or_default(),
        fs::read_to_string(paths.job_dir(job_id).join("supervisor.err")).unwrap_or_default(),
    );
    assert!(
        !supervisor_stderr.contains("belongs to durable Job"),
        "the trusted driver was refused by the public mutation fence"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn nested_plan_stays_under_one_job_with_one_time_delegations() {
    assert_nested_plan_case(None, false);
}

#[cfg(target_os = "macos")]
#[test]
fn nested_plan_recovers_after_merge_before_parent_result_is_recorded() {
    assert_nested_plan_case(Some("after_subplan_merge_before_parent_result"), true);
}

#[cfg(target_os = "macos")]
#[test]
fn campaign_reserved_plan_identity_survives_all_crash_windows() {
    for (failpoint, minimum_job_attempts) in [
        ("after_sub_launch_intent_before_spawn", 3),
        ("after_sub_plan_saved_before_ownership_freeze", 1),
        ("after_sub_plan_created_before_execution", 1),
        ("after_sub_merge_before_result", 1),
    ] {
        assert_campaign_recovery_case(failpoint, minimum_job_attempts);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn graph_root_mapping_is_repaired_after_creation_crash() {
    assert_root_mapping_recovery(JobShape::Graph);
}

#[cfg(target_os = "macos")]
#[test]
fn campaign_root_mapping_is_repaired_after_creation_crash() {
    assert_root_mapping_recovery(JobShape::LegacyCampaign);
}

#[cfg(target_os = "macos")]
fn assert_root_mapping_recovery(shape: JobShape) {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(workspace.join(".deadreckon")).expect("acceptance dir");
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(paths.config_path(), "default_provider = \"smoke\"\n").expect("config");
    fs::write(workspace.join("README.md"), "root mapping recovery\n").expect("readme");
    fs::write(
        workspace.join(".deadreckon/acceptance.yaml"),
        concat!(
            "name: root mapping recovery\n",
            "checks:\n",
            "  - kind: file_exists\n",
            "    path: \"{working_dir}/README.md\"\n",
        ),
    )
    .expect("acceptance");
    git(&workspace, &["init", "--initial-branch=main"]);
    git(
        &workspace,
        &["config", "user.email", "watchkeeper@example.invalid"],
    );
    git(&workspace, &["config", "user.name", "Watchkeeper Test"]);
    git(&workspace, &["add", "-A"]);
    git(&workspace, &["commit", "-m", "fixture"]);

    let output = match shape {
        JobShape::Graph => deadreckon(&paths, &workspace)
            .env(
                "DEADRECKON_TEST_PLAN_FAILPOINT",
                "after_root_plan_saved_before_driver_state",
            )
            .args([
                "start",
                "Recover the exact root graph after its mapping write is interrupted.",
                "--mode",
                "full-plan",
                "--children",
                "2",
                "--planner-provider",
                "smoke",
                "--provider",
                "smoke",
                "--max-spend",
                "2",
                "--yes",
                "--quiet",
                "--plain",
            ])
            .output()
            .expect("Graph start"),
        JobShape::LegacyCampaign => deadreckon(&paths, &workspace)
            .env("DEADRECKON_TEST_CAMPAIGN_FAILPOINTS", "1")
            .env(
                "DEADRECKON_TEST_CAMPAIGN_FAILPOINT",
                "after_root_campaign_saved_before_driver_state",
            )
            .args([
                "campaign",
                "Recover the exact root Campaign after its mapping write is interrupted.",
                "--n",
                "2",
                "--planner-provider",
                "smoke",
                "--provider",
                "smoke",
                "--max-spend",
                "2",
                "--max-wall-seconds",
                "120",
                "--yes",
                "--quiet",
                "--plain",
            ])
            .output()
            .expect("Campaign start"),
        _ => unreachable!(),
    };
    assert!(
        output.status.success(),
        "{shape:?}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let job_ids = directory_names(&paths.jobs_dir());
    assert_eq!(job_ids.len(), 1, "{shape:?}: {job_ids:?}");
    let job_id = job_ids.into_iter().next().expect("Job ID");

    let deadline = Instant::now() + Duration::from_secs(120);
    let view = loop {
        if let Ok(view) = JobView::load(&paths, &job_id)
            && view.projection.is_terminal()
        {
            break view;
        }
        assert!(
            Instant::now() < deadline,
            "{shape:?}: Job {job_id} did not terminate\nsupervisor:\n{}\ndriver:\n{}",
            fs::read_to_string(paths.job_dir(&job_id).join("supervisor-stderr.log"))
                .unwrap_or_default(),
            fs::read_to_string(paths.job_dir(&job_id).join("supervisor.err")).unwrap_or_default(),
        );
        thread::sleep(Duration::from_millis(50));
    };
    assert!(
        view.projection.attempt_count >= 2,
        "{shape:?}: root failpoint did not force recovery: {view:#?}"
    );
    let history = read_job_history(&paths.job_events(&job_id)).expect("Job history");
    assert!(
        history
            .events()
            .iter()
            .any(|event| event.kind == deadreckon_protocol::JobEventKind::RetryScheduled),
        "{shape:?}: no bounded recovery was scheduled"
    );
    assert_eq!(
        history
            .events()
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    deadreckon_protocol::JobEventKind::Verified
                        | deadreckon_protocol::JobEventKind::NeedsReview
                        | deadreckon_protocol::JobEventKind::Blocked
                        | deadreckon_protocol::JobEventKind::BudgetExhausted
                        | deadreckon_protocol::JobEventKind::DeadlineReached
                        | deadreckon_protocol::JobEventKind::Cancelled
                        | deadreckon_protocol::JobEventKind::Failed
                )
            })
            .count(),
        1,
        "{shape:?}: terminal history is not single-valued"
    );
    assert!(
        paths.job_dir(&job_id).join("driver.json").is_file(),
        "{shape:?}: root mapping was not repaired"
    );

    match shape {
        JobShape::Graph => {
            let plan = deadreckon_core::load_plan(&paths, &job_id).expect("recovered root Plan");
            assert_eq!(plan.plan_id, job_id);
            assert_eq!(plan.owner_job_id.as_deref(), Some(job_id.as_str()));
            let embedded = plan
                .root_planner_accounting
                .as_ref()
                .expect("embedded Plan accounting");
            let restored: deadreckon_core::plan::RootPlannerAccounting = serde_json::from_slice(
                &fs::read(paths.plan_dir(&job_id).join("root-planner-accounting.json"))
                    .expect("restored Plan accounting"),
            )
            .expect("Plan accounting JSON");
            assert_eq!(&restored, embedded);
        }
        JobShape::LegacyCampaign => {
            let campaign = deadreckon_core::campaign::read_campaign(&paths.plan_dir(&job_id))
                .expect("recovered root Campaign");
            assert_eq!(campaign.campaign_id, job_id);
            let embedded = campaign
                .root_planner_accounting
                .as_ref()
                .expect("embedded Campaign accounting");
            if embedded.planner_invoked {
                let event =
                    deadreckon_core::campaign::read_campaign_events(&paths.plan_dir(&job_id))
                        .expect("Campaign events")
                        .into_iter()
                        .rev()
                        .find(|event| event.kind == "root_planner_accounting")
                        .expect("restored Campaign accounting");
                assert_eq!(
                    event.detail.get("cost_usd").and_then(Value::as_f64),
                    Some(embedded.cost_usd)
                );
                assert_eq!(
                    event.detail.get("wall_seconds").and_then(Value::as_f64),
                    Some(embedded.wall_seconds)
                );
            }
        }
        _ => unreachable!(),
    }
    assert_eq!(
        fs::read_dir(paths.jobs_dir())
            .expect("jobs")
            .flatten()
            .count(),
        1,
        "{shape:?}: recovery created a second root Job"
    );
}

#[cfg(target_os = "macos")]
fn assert_campaign_recovery_case(failpoint: &str, minimum_job_attempts: u32) {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(workspace.join(".deadreckon")).expect("acceptance dir");
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(paths.config_path(), "default_provider = \"smoke\"\n").expect("config");
    fs::write(
        workspace.join("README.md"),
        "watchkeeper campaign recovery smoke\n",
    )
    .expect("readme");
    fs::write(
        workspace.join(".deadreckon/acceptance.yaml"),
        concat!(
            "name: campaign recovery\n",
            "checks:\n",
            "  - kind: file_exists\n",
            "    path: \"{working_dir}/README.md\"\n",
        ),
    )
    .expect("acceptance");
    git(&workspace, &["init", "--initial-branch=main"]);
    git(
        &workspace,
        &["config", "user.email", "watchkeeper@example.invalid"],
    );
    git(&workspace, &["config", "user.name", "Watchkeeper Test"]);
    git(&workspace, &["add", "-A"]);
    git(&workspace, &["commit", "-m", "fixture"]);

    let output = deadreckon(&paths, &workspace)
        .env("DEADRECKON_TEST_CAMPAIGN_FAILPOINTS", "1")
        .env("DEADRECKON_TEST_CAMPAIGN_FAILPOINT", failpoint)
        .args([
            "campaign",
            "Exercise two independent durable Campaign branches.",
            "--n",
            "2",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--max-spend",
            "2",
            "--max-wall-seconds",
            "120",
            "--yes",
            "--quiet",
            "--plain",
        ])
        .output()
        .expect("durable Campaign start");
    assert!(
        output.status.success(),
        "{failpoint}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let job_ids = directory_names(&paths.jobs_dir());
    assert_eq!(job_ids.len(), 1, "{failpoint}: {job_ids:?}");
    let job_id = job_ids.into_iter().next().expect("Campaign Job ID");

    let deadline = Instant::now() + Duration::from_secs(120);
    let view = loop {
        if let Ok(view) = JobView::load(&paths, &job_id)
            && view.projection.is_terminal()
        {
            break view;
        }
        assert!(
            Instant::now() < deadline,
            "{failpoint}: Campaign Job {job_id} did not terminate\n{}",
            fs::read_to_string(paths.job_dir(&job_id).join("supervisor.err")).unwrap_or_default()
        );
        thread::sleep(Duration::from_millis(50));
    };
    assert!(
        view.projection.attempt_count >= minimum_job_attempts,
        "{failpoint}: {view:#?}"
    );

    let campaign = deadreckon_core::campaign::read_campaign(&paths.plan_dir(&job_id))
        .unwrap_or_else(|error| {
            panic!(
                "{failpoint}: Campaign missing after recovery: {error}\nJob:\n{view:#?}\nDriver stderr:\n{}",
                fs::read_to_string(paths.job_dir(&job_id).join("supervisor.err"))
                    .unwrap_or_default()
            )
        });
    assert!(
        matches!(
            campaign.status,
            deadreckon_core::campaign::CampaignStatus::Merged
                | deadreckon_core::campaign::CampaignStatus::Failed
        ),
        "{failpoint}: Campaign did not reach a bounded post-child state\n{campaign:#?}\n{view:#?}"
    );
    let plan_ids = campaign
        .sub_goals
        .iter()
        .map(|sub| {
            assert_eq!(
                sub.status,
                deadreckon_core::campaign::SubGoalStatus::Merged,
                "{failpoint}: {sub:#?}"
            );
            assert!(sub.result_run_id.is_some(), "{failpoint}: {sub:#?}");
            sub.sub_plan_id.clone().expect("reserved sub Plan ID")
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(plan_ids.len(), 2, "{failpoint}: {campaign:#?}");
    for plan_id in &plan_ids {
        let plan = deadreckon_core::load_plan(&paths, plan_id).expect("recovered sub Plan");
        assert_eq!(plan.plan_id, *plan_id);
        assert_eq!(plan.owner_job_id.as_deref(), Some(job_id.as_str()));
        assert_eq!(plan.status, deadreckon_core::PlanStatus::Merged);
    }
    assert_eq!(
        fs::read_dir(paths.jobs_dir())
            .expect("jobs")
            .flatten()
            .count(),
        1,
        "{failpoint}: Campaign recovery created another root Job"
    );
}

#[cfg(target_os = "macos")]
fn assert_nested_plan_case(failpoint: Option<&str>, expect_recovery: bool) {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(workspace.join(".deadreckon")).expect("acceptance dir");
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(paths.config_path(), "default_provider = \"smoke\"\n").expect("config");
    fs::write(
        workspace.join("README.md"),
        "watchkeeper nested graph smoke\n",
    )
    .expect("readme");
    fs::write(
        workspace.join(".deadreckon/acceptance.yaml"),
        concat!(
            "name: nested graph driver\n",
            "checks:\n",
            "  - kind: file_exists\n",
            "    path: \"{working_dir}/README.md\"\n",
        ),
    )
    .expect("acceptance");
    git(&workspace, &["init", "--initial-branch=main"]);
    git(
        &workspace,
        &["config", "user.email", "watchkeeper@example.invalid"],
    );
    git(&workspace, &["config", "user.name", "Watchkeeper Test"]);
    git(&workspace, &["add", "-A"]);
    git(&workspace, &["commit", "-m", "fixture"]);

    let saved_plan = temp.path().join("nested-launch-plan.json");
    fs::write(
        &saved_plan,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": 1,
            "created_at": "2026-07-29T00:00:00Z",
            "goal": "Exercise the nested graph driver.",
            "shape": "plan",
            "n": 2,
            "pieces": [
                {
                    "id": "nested-project",
                    "goal": "Complete the nested project.",
                    "provider": "smoke",
                    "subplan": {
                        "apply": "at-end",
                        "pieces": [
                            {
                                "id": "nested-a",
                                "goal": "Complete nested part A.",
                                "provider": "smoke"
                            },
                            {
                                "id": "nested-b",
                                "goal": "Complete nested part B.",
                                "provider": "smoke"
                            }
                        ]
                    }
                },
                {
                    "id": "sibling",
                    "goal": "Complete the sibling task.",
                    "provider": "smoke",
                    "depends_on": ["nested-project"]
                }
            ],
            "providers": {
                "planner": "smoke",
                "coder": "smoke"
            },
            "budget": {
                "ceiling_usd": 1.0,
                "wall_seconds": 120
            },
            "contract": {
                "source": "operator"
            },
            "signals": {},
            "resolution": {
                "source": "operator",
                "confidence": 1.0,
                "rationale": "nested ownership integration fixture"
            }
        }))
        .expect("saved plan JSON"),
    )
    .expect("saved plan");

    let mut start = deadreckon(&paths, &workspace);
    start.args([
        "start",
        "--plan",
        saved_plan.to_str().expect("saved plan path"),
        "--yes",
        "--quiet",
        "--plain",
        "--json",
    ]);
    if let Some(failpoint) = failpoint {
        start.env("DEADRECKON_TEST_PLAN_FAILPOINT", failpoint);
    }
    let output = start.output().expect("nested graph start");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let job_ids = directory_names(&paths.jobs_dir());
    assert_eq!(
        job_ids.len(),
        1,
        "nested start must create exactly one durable Job; found {job_ids:?}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let job_id = job_ids.into_iter().next().expect("durable Job ID");

    let deadline = Instant::now() + Duration::from_secs(90);
    let view = loop {
        if let Ok(view) = JobView::load(&paths, &job_id)
            && view.projection.is_terminal()
        {
            break view;
        }
        assert!(
            Instant::now() < deadline,
            "nested Graph Job {job_id} did not terminate\n{}",
            fs::read_to_string(paths.job_dir(&job_id).join("supervisor.err")).unwrap_or_default()
        );
        thread::sleep(Duration::from_millis(50));
    };

    let root = deadreckon_core::load_plan(&paths, &job_id).unwrap_or_else(|error| {
        panic!(
            "root Plan missing: {error}\nJob:\n{view:#?}\nDriver stderr:\n{}",
            fs::read_to_string(paths.job_dir(&job_id).join("supervisor.err")).unwrap_or_default()
        )
    });
    let nested_id = root.tasks[0]
        .subplan
        .as_deref()
        .expect("root task retained nested Plan ID");
    let nested = deadreckon_core::load_plan(&paths, nested_id).expect("nested Plan");
    assert_eq!(root.owner_job_id.as_deref(), Some(job_id.as_str()));
    assert_eq!(nested.owner_job_id.as_deref(), Some(job_id.as_str()));
    assert_eq!(nested.parent_plan_id.as_deref(), Some(job_id.as_str()));
    assert_eq!(root.status, deadreckon_core::PlanStatus::Merged);
    assert_eq!(nested.status, deadreckon_core::PlanStatus::Merged);
    if expect_recovery {
        assert!(
            view.projection.attempt_count >= 2,
            "the failpoint did not force a durable Job retry: {view:#?}"
        );
        let history = read_job_history(&paths.job_events(&job_id)).expect("Job history");
        assert!(
            history
                .events()
                .iter()
                .any(|event| { event.kind == deadreckon_protocol::JobEventKind::RetryScheduled }),
            "the interrupted nested merge did not record a bounded retry"
        );
    }
    assert_eq!(
        fs::read_dir(paths.jobs_dir())
            .expect("jobs")
            .flatten()
            .count(),
        1,
        "nested execution created a second root Job"
    );
    assert!(
        fs::read_dir(paths.job_dir(&job_id).join("delegations").join("consumed"),)
            .expect("consumed delegations")
            .flatten()
            .count()
            >= 5,
        "nested execution did not consume task/fork/merge capabilities"
    );
}

fn write_job_fixture(
    paths: &DeadreckonPaths,
    workspace: &std::path::Path,
    job_id: &str,
    shape: JobShape,
) {
    write_job(
        paths,
        &Job {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(job_id.to_string()),
            scope: "watchkeeper-test".to_string(),
            goal: "job-owned artifact fixture".to_string(),
            shape,
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

fn deadreckon(paths: &DeadreckonPaths, workspace: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command
        .current_dir(workspace)
        .env("DEADRECKON_HOME", paths.home());
    command
}

fn assert_job_owned_refusal(output: &Output) {
    assert!(
        !output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("belongs to durable Job"), "{stderr}");
    assert!(stderr.contains("deadreckon attach"), "{stderr}");
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

fn git(cwd: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
