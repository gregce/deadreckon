use chrono::Utc;
use deadreckon_core::{
    DeadreckonPaths, PhaseId, PhaseStatus, RunOptions, create_run, load_run, promote_completed_run,
    save_state, write_acceptance_marker,
};
use tempfile::TempDir;

mod common;

use common::{deadreckon, stderr, stdout};

fn workdir(temp: &TempDir) -> std::path::PathBuf {
    let work = temp.path().join("work");
    std::fs::create_dir_all(&work).expect("workdir");
    work
}

#[test]
fn campaign_preview_writes_campaign_json_and_stops() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let work = workdir(&temp);

    let output = deadreckon(&paths)
        .current_dir(&work)
        .args([
            "campaign",
            "build a thing",
            "--n",
            "2",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--preview",
        ])
        .output()
        .expect("run campaign --preview");
    assert!(
        output.status.success(),
        "{}{}",
        stdout(&output),
        stderr(&output)
    );

    // A campaign.json was written, and the run stopped before any merge.
    let plans = paths.home().join("plans");
    let mut campaign_json = None;
    for entry in std::fs::read_dir(&plans).expect("plans dir") {
        let dir = entry.expect("entry").path();
        let candidate = dir.join("campaign.json");
        if candidate.is_file() {
            campaign_json = Some((dir, candidate));
        }
    }
    let (dir, candidate) =
        campaign_json.unwrap_or_else(|| panic!("campaign.json not written: {}", stdout(&output)));
    let body = std::fs::read_to_string(&candidate).expect("read campaign.json");
    assert!(body.contains("\"n\": 2"), "{body}");
    assert!(body.contains("\"status\": \"pending\""), "{body}");
    // Preview stops before fork: no merge-working dir.
    assert!(!dir.join("merge-working").exists());
}

#[test]
fn campaign_preflight_shows_depth_cap_and_tree_budget() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let work = workdir(&temp);

    let output = deadreckon(&paths)
        .current_dir(&work)
        .args([
            "campaign",
            "build a thing",
            "--n",
            "2",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--max-spend",
            "10",
            "--preview",
        ])
        .output()
        .expect("run campaign preflight");
    assert!(
        output.status.success(),
        "{}{}",
        stdout(&output),
        stderr(&output)
    );
    let out = stdout(&output);
    assert!(out.contains("depth cap 2"), "{out}");
    assert!(out.contains("tree budget $10"), "{out}");
}

#[test]
fn campaign_without_n_uses_recommended_count() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let work = workdir(&temp);

    let output = deadreckon(&paths)
        .current_dir(&work)
        .args([
            "campaign",
            "rebuild billing, notifications, and admin",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--preview",
        ])
        .output()
        .expect("run campaign --preview");
    assert!(
        output.status.success(),
        "{}{}",
        stdout(&output),
        stderr(&output)
    );

    let plans = paths.home().join("plans");
    let campaign_json = std::fs::read_dir(&plans)
        .expect("plans dir")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path().join("campaign.json"))
        .find(|path| path.is_file())
        .expect("campaign.json");
    let body = std::fs::read_to_string(campaign_json).expect("read campaign.json");
    assert!(body.contains("\"n\": 3"), "{body}");
    assert!(stdout(&output).contains("campaign: 3 sub-orchestrators"));
}

#[test]
fn campaign_rejects_n_outside_range_at_cli() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let work = workdir(&temp);

    for n in ["1", "7"] {
        let output = deadreckon(&paths)
            .current_dir(&work)
            .args([
                "campaign",
                "build a thing",
                "--n",
                n,
                "--planner-provider",
                "smoke",
                "--preview",
            ])
            .output()
            .expect("run campaign bad n");
        assert!(
            !output.status.success(),
            "--n {n} should be rejected: {}",
            stdout(&output)
        );
    }
}

fn promoted_run_with_file(
    paths: &DeadreckonPaths,
    cwd: &std::path::Path,
    goal: &str,
    relative: &str,
    body: &str,
) -> deadreckon_core::PipelineState {
    let mut state = create_run(
        paths,
        RunOptions {
            goal: goal.to_string(),
            cwd: cwd.to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some("smoke".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("create run");
    let file = state.working_dir.join(relative);
    std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
    std::fs::write(&file, body).expect("write file");
    write_acceptance_marker(
        &state.run_root,
        state.run_id.clone(),
        state.working_dir.clone(),
        1,
    )
    .expect("marker");
    state
        .set_phase_status(PhaseId(60), PhaseStatus::Completed)
        .expect("complete");
    save_state(&state).expect("save");
    promote_completed_run(paths, &mut state).expect("promote");
    state
}

#[test]
fn campaign_repair_promotes_failed_campaign_without_conflicts() {
    use deadreckon_core::campaign::{
        Campaign, CampaignStatus, SubGoalStatus, build_rollup, build_sub_goals, read_campaign,
        write_campaign, write_campaign_rollup,
    };
    use deadreckon_core::plan::PlanProviders;

    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let work = workdir(&temp);
    let run0 = promoted_run_with_file(&paths, &work, "billing", "src/billing.rs", "billing");
    let run1 = promoted_run_with_file(&paths, &work, "notify", "src/notify.rs", "notify");
    let mut campaign = Campaign::new(
        "ship billing and notifications",
        build_sub_goals(
            vec!["ship billing".to_string(), "ship notifications".to_string()],
            2,
        )
        .expect("subs"),
        PlanProviders::default(),
        0,
        None,
        None,
        "test",
    )
    .expect("campaign");
    campaign.status = CampaignStatus::Failed;
    campaign.forked_at = Some(Utc::now());
    campaign.sub_goals[0].status = SubGoalStatus::Merged;
    campaign.sub_goals[0].result_run_id = Some(run0.run_id.clone());
    campaign.sub_goals[0].scope = Some(run0.scope.clone());
    campaign.sub_goals[1].status = SubGoalStatus::Merged;
    campaign.sub_goals[1].result_run_id = Some(run1.run_id.clone());
    campaign.sub_goals[1].scope = Some(run1.scope.clone());
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    std::fs::create_dir_all(&campaign_dir).expect("campaign dir");
    write_campaign(&campaign_dir, &campaign).expect("write campaign");
    let rollup = build_rollup(&campaign, |_| {
        (
            "signed".to_string(),
            deadreckon_core::tamper::AcceptanceTamperVerdict::Clean,
            Vec::new(),
        )
    });
    write_campaign_rollup(&campaign_dir, &rollup).expect("write rollup");

    let output = deadreckon(&paths)
        .current_dir(&work)
        .args(["campaign", "repair", &campaign.campaign_id, "--quiet"])
        .output()
        .expect("repair campaign");
    assert!(
        output.status.success(),
        "{}{}",
        stdout(&output),
        stderr(&output)
    );

    let repaired = read_campaign(&campaign_dir).expect("read repaired campaign");
    assert_eq!(repaired.status, CampaignStatus::Merged);
    let result_run_id = repaired.merged_run_id.expect("merged run id");
    let result = load_run(&paths, &result_run_id).expect("load result");
    let library = paths.library_dir(&result.scope, &result.run_id);
    assert_eq!(
        std::fs::read_to_string(library.join("src/billing.rs")).expect("billing"),
        "billing"
    );
    assert_eq!(
        std::fs::read_to_string(library.join("src/notify.rs")).expect("notify"),
        "notify"
    );
}
