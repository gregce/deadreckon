#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use deadreckon_core::DeadreckonPaths;
use deadreckon_core::campaign::{
    Campaign, CampaignStatus, SubGoalStatus, build_rollup, build_sub_goals, write_campaign,
    write_campaign_rollup,
};
use deadreckon_core::plan::{
    Plan, PlanMode, PlanProviders, PlanRole, PlanStatus, PlanTask, PlanTaskStatus, save_plan,
};
use deadreckon_core::tamper::AcceptanceTamperVerdict;
use tempfile::TempDir;

#[test]
fn unowned_legacy_merge_refuses_before_any_artifact_mutation() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");

    let mut plan = Plan::new(
        "complete both legacy branches",
        PlanMode::FullPlan,
        vec![
            PlanTask::new(0, "first", "complete first", PlanRole::Child, None),
            PlanTask::new(1, "second", "complete second", PlanRole::Child, None),
        ],
        PlanProviders::default(),
        Some("legacy-merge-test".to_string()),
        "test",
    )
    .expect("legacy Plan");
    plan.status = PlanStatus::Forked;
    plan.parent_cwd = Some(workspace.clone());
    for task in &mut plan.tasks {
        task.status = PlanTaskStatus::Completed;
    }
    save_plan(&paths, &plan).expect("save legacy Plan");

    let before = snapshot_tree(paths.home());
    let output = deadreckon(&paths, &workspace)
        .args([
            "merge",
            &plan.plan_id,
            "--strategy",
            "prefer-child",
            "--prefer-child",
            "999",
            "--repair-provider",
            "must-not-be-resolved",
            "--quiet",
            "--plain",
        ])
        .output()
        .expect("legacy merge");

    assert_unowned_refusal(&output, "merge", "Plan");
    assert_eq!(
        snapshot_tree(paths.home()),
        before,
        "merge changed state, events, provider evidence, or result files before refusing"
    );
}

#[test]
fn unowned_legacy_campaign_repair_refuses_before_any_artifact_mutation() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");

    let mut campaign = Campaign::new(
        "complete both legacy campaign branches",
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
    .expect("legacy Campaign");
    campaign.status = CampaignStatus::Failed;
    for (index, sub) in campaign.sub_goals.iter_mut().enumerate() {
        sub.status = SubGoalStatus::Merged;
        sub.result_run_id = Some(format!("{:032}", index + 1));
        sub.scope = Some("legacy-campaign-test".to_string());
    }
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    fs::create_dir_all(&campaign_dir).expect("campaign dir");
    write_campaign(&campaign_dir, &campaign).expect("write legacy Campaign");
    let rollup = build_rollup(&campaign, |_| {
        (
            "signed".to_string(),
            AcceptanceTamperVerdict::Clean,
            Vec::new(),
        )
    });
    write_campaign_rollup(&campaign_dir, &rollup).expect("write campaign roll-up");

    let before = snapshot_tree(paths.home());
    let output = deadreckon(&paths, &workspace)
        .args([
            "campaign",
            "repair",
            &campaign.campaign_id,
            "--repair-mode",
            "child",
            "--repair-provider",
            "must-not-be-resolved",
            "--quiet",
            "--plain",
        ])
        .output()
        .expect("legacy campaign repair");

    assert_unowned_refusal(&output, "campaign repair", "Campaign");
    assert_eq!(
        snapshot_tree(paths.home()),
        before,
        "campaign repair changed state, events, provider evidence, or result files before refusing"
    );
}

fn deadreckon(paths: &DeadreckonPaths, workspace: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command
        .current_dir(workspace)
        .env("DEADRECKON_HOME", paths.home());
    command
}

fn assert_unowned_refusal(output: &Output, operation: &str, artifact_kind: &str) {
    assert!(
        !output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(operation), "{stderr}");
    assert!(stderr.contains(artifact_kind), "{stderr}");
    assert!(stderr.contains("has no durable Job owner"), "{stderr}");
    assert!(stderr.contains("deadreckon start"), "{stderr}");
    assert!(!stderr.contains("--untrusted"), "{stderr}");
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, current: &Path, snapshot: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        let mut entries = fs::read_dir(current)
            .expect("read snapshot directory")
            .collect::<Result<Vec<_>, _>>()
            .expect("read snapshot entries");
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("snapshot path stays under root")
                .to_path_buf();
            let file_type = entry.file_type().expect("snapshot file type");
            if file_type.is_dir() {
                snapshot.insert(relative, None);
                visit(root, &path, snapshot);
            } else {
                snapshot.insert(relative, Some(fs::read(&path).expect("snapshot file")));
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    if root.is_dir() {
        visit(root, root, &mut snapshot);
    }
    snapshot
}
