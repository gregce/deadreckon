use std::collections::HashMap;

use super::commands::campaign::*;
use super::*;

#[test]
fn sub_orchestrator_launch_sets_lineage_env_and_isolated_scope() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let home = tmp.path().join("home");
    let source = tmp.path().join("src");
    let launch_dir = tmp.path().join("plans/camp-1/launch/sub-0");
    let ancestors = vec!["root-key".to_string()];
    let launch = CampaignSubLaunch {
        home: &home,
        source_dir: &source,
        launch_dir: &launch_dir,
        campaign_id: "camp-1",
        sub_goal: "rebuild billing",
        sub_n: 2,
        sandbox: "none",
        max_spend: Some(5.0),
        plain: true,
        planner_provider: Some("smoke"),
        child_provider: Some("smoke"),
        ancestor_task_keys: &ancestors,
        ancestor_scopes: &[],
    };
    let command = build_sub_orchestrator_command(&launch).expect("command");

    let envs: HashMap<String, Option<String>> = command
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect();
    assert_eq!(
        envs.get("DEADRECKON_CAMPAIGN_DEPTH"),
        Some(&Some("1".to_string()))
    );
    assert_eq!(
        envs.get("DEADRECKON_CAMPAIGN_ROOT"),
        Some(&Some("camp-1".to_string()))
    );
    assert_eq!(
        envs.get("DEADRECKON_SCOPE_ROOT").and_then(|v| v.clone()),
        Some(launch_dir.to_string_lossy().into_owned())
    );
    assert_eq!(
        envs.get("DEADRECKON_CAMPAIGN_SUB_RESULT")
            .and_then(|v| v.clone()),
        Some(launch_dir.to_string_lossy().into_owned())
    );

    let args: Vec<String> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert!(args.contains(&"orchestrate".to_string()));
    assert!(args.contains(&"full-plan".to_string()));
    assert!(args.contains(&"rebuild billing".to_string()));
    assert!(args.contains(&"--yes".to_string()));
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "--n" && pair[1] == "2")
    );
}

#[test]
fn sub_orchestrator_result_run_is_discovered_from_launch_sidecar() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let launch_dir = tmp.path().join("plans/camp-1/launch/sub-0");
    let result = deadreckon_core::campaign::SubResult {
        schema_version: 1,
        sub_id: "sub-0".to_string(),
        plan_id: Some("plan-7".to_string()),
        result_run_id: Some("run-42".to_string()),
        ok: true,
    };
    deadreckon_core::campaign::write_sub_result(&launch_dir, &result).expect("write sidecar");

    let discovered = discover_sub_result(&launch_dir)
        .expect("discover")
        .expect("sidecar present");
    assert_eq!(discovered.result_run_id.as_deref(), Some("run-42"));
    assert_eq!(discovered.plan_id.as_deref(), Some("plan-7"));
}

fn fixture_campaign() -> deadreckon_core::campaign::Campaign {
    let subs = deadreckon_core::campaign::build_sub_goals(
        vec![
            "rebuild billing".to_string(),
            "rebuild notifications".to_string(),
        ],
        2,
    )
    .expect("subs");
    deadreckon_core::campaign::Campaign::new(
        "rebuild billing and notifications",
        subs,
        deadreckon_core::plan::PlanProviders::default(),
        0,
        Some(10.0),
        None,
        "0.1.0",
    )
    .expect("campaign")
}

#[test]
fn campaign_fork_launches_all_subs_and_records_events() {
    use deadreckon_core::campaign::{CampaignStatus, SubGoalStatus, SubResult};
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let campaign_dir = tmp.path().join("plans").join("camp-1");
    let mut campaign = fixture_campaign();

    let mut launched = Vec::new();
    run_campaign_fork(
        &campaign_dir,
        &mut campaign,
        |sub, _launch_dir| {
            launched.push(sub.sub_id.clone());
            Ok(SubResult {
                schema_version: 1,
                sub_id: sub.sub_id.clone(),
                plan_id: Some(format!("plan-{}", sub.sub_id)),
                result_run_id: Some(format!("run-{}", sub.sub_id)),
                ok: true,
            })
        },
        |_| 0.0,
    )
    .expect("fork");

    assert_eq!(launched, vec!["sub-0", "sub-1"]);
    assert_eq!(campaign.status, CampaignStatus::Forked);
    assert!(
        campaign
            .sub_goals
            .iter()
            .all(|s| s.status == SubGoalStatus::Merged)
    );
    assert_eq!(
        campaign.sub_goals[0].result_run_id.as_deref(),
        Some("run-sub-0")
    );

    let events = deadreckon_core::campaign::read_campaign_events(&campaign_dir).expect("events");
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds.iter().filter(|k| **k == "sub_launched").count(), 2);
    assert_eq!(kinds.iter().filter(|k| **k == "sub_merged").count(), 2);
    assert!(kinds.contains(&"campaign_started"));
}

#[test]
fn campaign_fork_marks_failed_sub_without_aborting_siblings() {
    use deadreckon_core::campaign::{SubGoalStatus, SubResult};
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let campaign_dir = tmp.path().join("plans").join("camp-2");
    let mut campaign = fixture_campaign();

    let mut launched = Vec::new();
    run_campaign_fork(
        &campaign_dir,
        &mut campaign,
        |sub, _launch_dir| {
            launched.push(sub.sub_id.clone());
            if sub.sub_id == "sub-0" {
                Err(CliError::Core(DeadreckonError::InvalidInput(
                    "sub-0 blew up".to_string(),
                )))
            } else {
                Ok(SubResult {
                    schema_version: 1,
                    sub_id: sub.sub_id.clone(),
                    plan_id: Some("plan-sub-1".to_string()),
                    result_run_id: Some("run-sub-1".to_string()),
                    ok: true,
                })
            }
        },
        |_| 0.0,
    )
    .expect("fork continues past failure");

    // The sibling still launched despite sub-0 failing.
    assert_eq!(launched, vec!["sub-0", "sub-1"]);
    assert_eq!(campaign.sub_goals[0].status, SubGoalStatus::Failed);
    assert_eq!(campaign.sub_goals[1].status, SubGoalStatus::Merged);
    assert_eq!(
        campaign.sub_goals[1].result_run_id.as_deref(),
        Some("run-sub-1")
    );
}

#[test]
fn aggregate_spend_exhaustion_refuses_next_sub_launch() {
    use deadreckon_core::campaign::SubResult;
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let campaign_dir = tmp.path().join("plans").join("camp-3");
    let mut campaign = fixture_campaign(); // tree_budget_usd = 10.0, 2 subs

    let mut launched = Vec::new();
    run_campaign_fork(
        &campaign_dir,
        &mut campaign,
        |sub, _launch_dir| {
            launched.push(sub.sub_id.clone());
            Ok(SubResult {
                schema_version: 1,
                sub_id: sub.sub_id.clone(),
                plan_id: Some(format!("plan-{}", sub.sub_id)),
                result_run_id: Some(format!("run-{}", sub.sub_id)),
                ok: true,
            })
        },
        // sub-0 alone spends the whole $10 tree budget.
        |_| 12.0,
    )
    .expect("fork");

    // sub-1 is never launched because the tree budget is exhausted after sub-0.
    assert_eq!(launched, vec!["sub-0"]);
    let events = deadreckon_core::campaign::read_campaign_events(&campaign_dir).expect("events");
    assert!(events.iter().any(|e| e.kind == "budget_exhausted"));
}

fn write_file(root: &std::path::Path, relative: &str, body: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, body).expect("write");
}

#[test]
fn compose_result_runs_extracted_without_changing_plan_merge() {
    // The shared enumeration plan merge relies on: lists real files, skips
    // internal/generated paths (.deadreckon, docs/RUN-*).
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let root = tmp.path().join("run-root");
    write_file(&root, "src/lib.rs", "pub fn a() {}");
    write_file(&root, ".deadreckon/docs/RUN-NARRATIVE.md", "internal");
    write_file(&root, "docs/RUN-AS-BUILT.md", "internal");
    write_file(&root, "docs/guide.md", "kept");

    let files = mergeable_run_files(&root).expect("files");
    let relatives: Vec<String> = files
        .iter()
        .map(|(relative, _, _)| relative.to_string_lossy().into_owned())
        .collect();
    assert!(relatives.iter().any(|r| r == "src/lib.rs"));
    assert!(relatives.iter().any(|r| r == "docs/guide.md"));
    assert!(!relatives.iter().any(|r| r.contains(".deadreckon")));
    assert!(!relatives.iter().any(|r| r.contains("RUN-")));
}

#[test]
fn campaign_meta_merge_composes_two_clean_sub_results() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let root0 = tmp.path().join("sub-0-result");
    let root1 = tmp.path().join("sub-1-result");
    write_file(&root0, "src/billing.rs", "billing");
    write_file(&root1, "src/notify.rs", "notify");
    let merge_dir = tmp.path().join("merge-working");

    let result = compose_roots(
        &[("run-0".to_string(), root0), ("run-1".to_string(), root1)],
        &merge_dir,
    )
    .expect("compose");

    assert!(result.conflicts.is_empty());
    assert!(merge_dir.join("src/billing.rs").is_file());
    assert!(merge_dir.join("src/notify.rs").is_file());
}

#[test]
fn cross_sub_file_conflict_fails_campaign() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let root0 = tmp.path().join("sub-0-result");
    let root1 = tmp.path().join("sub-1-result");
    write_file(&root0, "src/shared.rs", "version from sub 0");
    write_file(&root1, "src/shared.rs", "version from sub 1");
    let merge_dir = tmp.path().join("merge-working");

    let result = compose_roots(
        &[("run-0".to_string(), root0), ("run-1".to_string(), root1)],
        &merge_dir,
    )
    .expect("compose");

    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(
        result.conflicts[0].path,
        std::path::PathBuf::from("src/shared.rs")
    );
}

fn campaign_with_rollup() -> (
    deadreckon_core::campaign::Campaign,
    deadreckon_core::campaign::CampaignRollup,
) {
    use deadreckon_core::campaign::build_rollup;
    let mut campaign = fixture_campaign();
    campaign.sub_goals[0].result_run_id = Some("run-0".to_string());
    campaign.sub_goals[0].sub_plan_id = Some("plan-0".to_string());
    campaign.sub_goals[1].result_run_id = Some("run-1".to_string());
    campaign.sub_goals[1].sub_plan_id = Some("plan-1".to_string());
    let rollup = build_rollup(&campaign, |run_id| {
        if run_id == "run-0" {
            (
                "refused".to_string(),
                deadreckon_core::tamper::AcceptanceTamperVerdict::Refuse,
                Vec::new(),
            )
        } else {
            (
                "signed".to_string(),
                deadreckon_core::tamper::AcceptanceTamperVerdict::Caveat,
                vec!["agent modified tests/x.rs".to_string()],
            )
        }
    });
    (campaign, rollup)
}

#[test]
fn why_failed_reports_refused_and_caveat_subs() {
    let (campaign, rollup) = campaign_with_rollup();
    let report = campaign_why_failed_report(&campaign, Some(&rollup));
    assert!(report.contains("refused subs: sub-0"), "{report}");
    assert!(report.contains("caveat subs: sub-1"), "{report}");
    assert!(report.contains("refused"), "{report}");
}

#[test]
fn campaign_attach_lists_subs_with_rollup_and_breadcrumb() {
    let (campaign, rollup) = campaign_with_rollup();
    let summary = campaign_attach_summary(None, &campaign, Some(&rollup));
    assert!(summary.contains("campaign:"), "{summary}");
    assert!(summary.contains("sub-0"), "{summary}");
    assert!(summary.contains("sub-1"), "{summary}");
    assert!(summary.contains("roll-up refused"), "{summary}");
}

#[test]
fn kill_campaign_cascades_to_sub_coordinators_and_children() {
    let (campaign, _rollup) = campaign_with_rollup();
    let targets = campaign_kill_targets(&campaign);
    assert_eq!(targets, vec!["plan-0", "plan-1"]);
}
