use tempfile::TempDir;

use super::commands::campaign::campaign_attach_summary;
use super::*;

fn subscription_state(temp: &TempDir) -> (DeadreckonPaths, deadreckon_core::PipelineState) {
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let cwd = temp.path().join("repo");
    std::fs::create_dir_all(&cwd).expect("repo");
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: "ship effortless consistency".to_string(),
            cwd,
            sandbox: "none".to_string(),
            provider: Some("cli:test".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(10.0),
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");
    state.turn = 2;
    state
        .set_phase_status(PhaseId(60), PhaseStatus::Completed)
        .expect("complete");
    save_state(&state).expect("save");
    deadreckon_core::append_spend(
        &state,
        &SpendRecord {
            timestamp: Utc::now(),
            turn: 1,
            provider: "cli:test".to_string(),
            model: "subscription".to_string(),
            input_tokens: 10,
            output_tokens: 14,
            cost_usd: 0.0,
            total_cost_usd: 0.0,
            cap_usd: Some(10.0),
            subscription: true,
            estimated: false,
            wall_time_seconds: Some(3.0),
            wall_time_cap_seconds: None,
        },
    )
    .expect("spend");
    deadreckon_core::write_acceptance_marker_with_results(
        &state.run_root,
        state.run_id.clone(),
        state.working_dir.clone(),
        vec![
            deadreckon_core::AcceptanceCheckResult {
                kind: "file_exists".to_string(),
                passed: true,
                must_pass: true,
                detail: "app.txt exists".to_string(),
                command: None,
                cwd: Some(state.working_dir.clone()),
                duration_ms: Some(1),
                stdout: None,
                stderr: None,
            },
            deadreckon_core::AcceptanceCheckResult {
                kind: "shell".to_string(),
                passed: true,
                must_pass: true,
                detail: "cargo test exited with exit status: 0".to_string(),
                command: Some("cargo test".to_string()),
                cwd: Some(state.working_dir.clone()),
                duration_ms: Some(2),
                stdout: None,
                stderr: None,
            },
        ],
    )
    .expect("acceptance marker");
    (paths, state)
}

fn plan_for_state(state: &deadreckon_core::PipelineState) -> Plan {
    let mut task = PlanTask::new(
        0,
        "polish result surface",
        "polish result surface",
        PlanRole::Child,
        state.provider.clone(),
    );
    task.status = PlanTaskStatus::Completed;
    task.child_run_id = Some(state.run_id.clone());
    let second = PlanTask::new(
        1,
        "verify companion surface",
        "verify companion surface",
        PlanRole::Child,
        state.provider.clone(),
    );
    Plan::new(
        "ship effortless consistency",
        PlanMode::FullPlan,
        vec![task, second],
        PlanProviders::default(),
        Some(state.scope.clone()),
        "0.1.0",
    )
    .expect("plan")
}

fn campaign_for_state(
    state: &deadreckon_core::PipelineState,
) -> deadreckon_core::campaign::Campaign {
    let subs = deadreckon_core::campaign::build_sub_goals(
        vec![
            "polish result surface".to_string(),
            "verify companion surface".to_string(),
        ],
        2,
    )
    .expect("subs");
    let mut campaign = deadreckon_core::campaign::Campaign::new(
        "ship effortless consistency",
        subs,
        PlanProviders::default(),
        0,
        None,
        None,
        "0.1.0",
    )
    .expect("campaign");
    campaign.status = deadreckon_core::campaign::CampaignStatus::Merged;
    campaign.sub_goals[0].status = deadreckon_core::campaign::SubGoalStatus::Merged;
    campaign.sub_goals[0].result_run_id = Some(state.run_id.clone());
    campaign
}

#[test]
fn no_surface_renders_zero_dollar_subscription_spend() {
    let temp = TempDir::new().expect("tempdir");
    let (paths, state) = subscription_state(&temp);
    let exit = render_exit_summary_card(&state, &RunLoopOutcome::Done, true, true);
    let plan = plan_for_state(&state);
    let plan_detail = plan_task_detail_lines(&paths, &plan, &plan.tasks[0], 120).join("\n");
    let campaign = campaign_for_state(&state);
    let campaign_summary = campaign_attach_summary(Some(&paths), &campaign, None);

    for (surface, rendered) in [
        ("exit", exit),
        ("plan", plan_detail),
        ("campaign", campaign_summary),
    ] {
        assert!(
            rendered.contains("not metered (subscription)"),
            "{surface} did not render honest subscription spend:\n{rendered}"
        );
        assert!(
            !rendered.contains("~$0.000000") && !rendered.contains("spend $0.000000"),
            "{surface} rendered zero-dollar metered spend:\n{rendered}"
        );
    }
}

#[test]
fn gate_verdict_is_per_check_on_every_outcome_surface() {
    let temp = TempDir::new().expect("tempdir");
    let (paths, state) = subscription_state(&temp);
    let exit = render_exit_summary_card(&state, &RunLoopOutcome::Done, true, true);
    let status = acceptance_status_line(&state);
    let plan = plan_for_state(&state);
    let plan_detail = plan_task_detail_lines(&paths, &plan, &plan.tasks[0], 120).join("\n");
    let campaign = campaign_for_state(&state);
    let campaign_summary = campaign_attach_summary(Some(&paths), &campaign, None);

    for (surface, rendered) in [
        ("exit", exit),
        ("status", status),
        ("plan", plan_detail),
        ("campaign", campaign_summary),
    ] {
        assert!(
            rendered.contains("gate: PASSED 2/2"),
            "{surface} did not render per-check gate verdict:\n{rendered}"
        );
        assert!(
            !rendered.contains("gate gate:"),
            "{surface} duplicated the gate label:\n{rendered}"
        );
    }
}
