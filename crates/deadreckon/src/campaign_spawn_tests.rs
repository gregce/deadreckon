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
        sub_plan_id: "00000000000000000000000000000001",
        sub_goal: "rebuild billing",
        sub_n: 2,
        sandbox: "none",
        max_spend: Some(5.0),
        max_wall_seconds: Some(30.0),
        plain: true,
        planner_provider: Some("smoke"),
        child_provider: Some("smoke"),
        reviewer_provider: Some("cli:codex"),
        planner_model: Some("planner-mx"),
        child_model: None,
        reviewer_model: Some("reviewer-mx"),
        narrate: false,
        narrator_model: None,
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
    assert_eq!(
        envs.get("DEADRECKON_CAMPAIGN_SUB_PLAN_ID")
            .and_then(|v| v.clone()),
        Some("00000000000000000000000000000001".to_string())
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
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "--max-spend" && pair[1] == "5.000000")
    );
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "--max-wall-seconds" && pair[1] == "30.000000")
    );
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "--reviewer-provider" && pair[1] == "cli:codex")
    );
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "--reviewer-model" && pair[1] == "reviewer-mx")
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
                plan_id: sub.sub_plan_id.clone(),
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
fn interrupted_campaign_resume_reconciles_linked_plan_without_duplicate_launch() {
    use deadreckon_core::campaign::{CampaignStatus, SubGoalStatus, SubResult};
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let campaign_dir = tmp.path().join("plans").join("camp-resume");
    let mut campaign = fixture_campaign();
    campaign.campaign_id = "camp-resume".to_string();
    campaign.status = CampaignStatus::Forked;
    campaign.sub_goals[0].status = SubGoalStatus::Running;
    campaign.sub_goals[0].sub_plan_id = Some("plan-sub-0".to_string());
    deadreckon_core::campaign::write_campaign(&campaign_dir, &campaign)
        .expect("persist interrupted campaign");

    let mut recovered = Vec::new();
    let mut launched = Vec::new();
    run_campaign_fork_with_recovery(
        &campaign_dir,
        &mut campaign,
        |sub, _launch_dir| {
            if sub.sub_plan_id.as_deref() == Some("plan-sub-0") {
                recovered.push(sub.sub_id.clone());
                return Ok(Some(SubResult {
                    schema_version: 1,
                    sub_id: sub.sub_id.clone(),
                    plan_id: sub.sub_plan_id.clone(),
                    result_run_id: Some("run-sub-0".to_string()),
                    ok: true,
                }));
            }
            Ok(None)
        },
        |sub, _launch_dir| {
            launched.push(sub.sub_id.clone());
            Ok(SubResult {
                schema_version: 1,
                sub_id: sub.sub_id.clone(),
                plan_id: sub.sub_plan_id.clone(),
                result_run_id: Some(format!("run-{}", sub.sub_id)),
                ok: true,
            })
        },
        |_| 1.0,
    )
    .expect("resume persisted campaign");

    assert_eq!(campaign.campaign_id, "camp-resume");
    assert_eq!(recovered, vec!["sub-0"]);
    assert_eq!(launched, vec!["sub-1"]);
    assert_eq!(
        campaign.sub_goals[0].result_run_id.as_deref(),
        Some("run-sub-0")
    );
    assert!(
        campaign
            .sub_goals
            .iter()
            .all(|sub| sub.status == SubGoalStatus::Merged)
    );
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
                    plan_id: sub.sub_plan_id.clone(),
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
                plan_id: sub.sub_plan_id.clone(),
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
fn mergeable_run_files_shared_without_changing_plan_merge() {
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
fn compose_helper_extracted_without_changing_merge_outcomes() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let root0 = tmp.path().join("sub-0-result");
    let root1 = tmp.path().join("sub-1-result");
    let root2 = tmp.path().join("sub-2-result");
    write_file(&root0, "src/shared.rs", "version from sub 0");
    write_file(&root0, "src/same.rs", "same content");
    write_file(&root1, "src/same.rs", "same content");
    write_file(&root1, "src/notify.rs", "notify");
    write_file(&root2, "src/shared.rs", "version from sub 2");
    let merge_dir = tmp.path().join("merge-working");
    let sources = vec![
        ComposeFileSource {
            root: root0,
            data: "run-0".to_string(),
            prefix_error: "merge source prefix error",
        },
        ComposeFileSource {
            root: root1,
            data: "run-1".to_string(),
            prefix_error: "merge source prefix error",
        },
        ComposeFileSource {
            root: root2,
            data: "run-2".to_string(),
            prefix_error: "merge source prefix error",
        },
    ];

    let conflicts = compose_merge_sources(
        &merge_dir,
        &sources,
        |label, _relative, _file, _hash| label.clone(),
        |relative, previous, current| ComposeMergeDecision::RecordConflict {
            conflict: ComposeConflict {
                path: relative.to_path_buf(),
                first_label: previous.clone(),
                second_label: current.clone(),
            },
            use_current: false,
        },
    )
    .expect("compose");

    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].path, std::path::PathBuf::from("src/shared.rs"));
    assert_eq!(conflicts[0].first_label, "run-0");
    assert_eq!(conflicts[0].second_label, "run-2");
    assert_eq!(
        fs::read_to_string(merge_dir.join("src/shared.rs")).expect("shared"),
        "version from sub 0"
    );
    assert_eq!(
        fs::read_to_string(merge_dir.join("src/same.rs")).expect("same"),
        "same content"
    );
    assert!(merge_dir.join("src/notify.rs").is_file());
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
fn cross_sub_file_conflict_is_recorded_for_repair() {
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

#[test]
fn cross_sub_file_conflict_accepts_synthesized_repair() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let root0 = tmp.path().join("sub-0-result");
    let root1 = tmp.path().join("sub-1-result");
    write_file(&root0, "src/shared.rs", "version from sub 0\n");
    write_file(&root1, "src/shared.rs", "version from sub 1\n");
    let merge_dir = tmp.path().join("merge-working");

    let mut task0 = PlanTask::new(0, "sub-0 foundation", "foundation", PlanRole::Child, None);
    task0.task_id = "sub-0".to_string();
    task0.child_run_id = Some("run-0".to_string());
    task0.status = PlanTaskStatus::Completed;
    let mut task1 = PlanTask::new(1, "sub-1 polish", "polish", PlanRole::Child, None);
    task1.task_id = "sub-1".to_string();
    task1.child_run_id = Some("run-1".to_string());
    task1.status = PlanTaskStatus::Completed;
    let mut plan = Plan::new(
        "campaign root",
        PlanMode::FullPlan,
        vec![task0, task1],
        PlanProviders::default(),
        None,
        "test",
    )
    .expect("plan");
    plan.plan_id = "campaign-1".to_string();

    let sources = vec![
        ComposeFileSource {
            root: root0.clone(),
            data: PlanMergeSource {
                task_id: "sub-0".to_string(),
                task_index: 0,
                run_id: "run-0".to_string(),
                artifact_root: root0,
            },
            prefix_error: "merge source prefix error",
        },
        ComposeFileSource {
            root: root1.clone(),
            data: PlanMergeSource {
                task_id: "sub-1".to_string(),
                task_index: 1,
                run_id: "run-1".to_string(),
                artifact_root: root1,
            },
            prefix_error: "merge source prefix error",
        },
    ];
    let conflicts = compose_merge_sources(
        &merge_dir,
        &sources,
        |source, _relative, file, hash| PlanMergeSeenFile {
            task_id: source.task_id.clone(),
            task_index: source.task_index,
            run_id: source.run_id.clone(),
            artifact_root: source.artifact_root.clone(),
            artifact_path: file.to_path_buf(),
            hash,
        },
        |relative, previous, current| ComposeMergeDecision::RecordConflict {
            conflict: plan_merge_conflict(&plan, relative, previous, current, None),
            use_current: false,
        },
    )
    .expect("compose");
    let mut merge = PlanMergeOutcome {
        working_dir: merge_dir.clone(),
        conflicts,
    };
    let repair_plan = MergeRepairPlan {
        schema_version: Some(1),
        decision: "synthesize".to_string(),
        rationale: "combine campaign edits".to_string(),
        actions: vec![MergeRepairAction {
            path: std::path::PathBuf::from("src/shared.rs"),
            action: "write_synthesized".to_string(),
            chosen_task_id: None,
            content: Some("merged campaign version\n".to_string()),
            preserve: Vec::new(),
        }],
        repair_goal: None,
        planner_spend_usd: 0.0,
        planner_wall_seconds: 0.0,
    };

    validate_merge_repair_plan(
        &repair_plan,
        &merge.unresolved_conflicts(),
        MergeRepairMode::Auto,
    )
    .expect("repair plan valid");
    apply_synthesized_repair(&repair_plan, &mut merge).expect("repair");

    assert_eq!(
        fs::read_to_string(merge_dir.join("src/shared.rs")).expect("shared"),
        "merged campaign version\n"
    );
    assert_eq!(
        merge.conflicts[0]
            .deterministic_resolution
            .as_ref()
            .map(|resolution| resolution.kind.as_str()),
        Some("planner_synthesize")
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
    let report = campaign_why_failed_report(
        &DeadreckonPaths::from_home(std::env::temp_dir().join("dr-test-none")),
        &campaign,
        Some(&rollup),
    );
    assert!(report.contains("refused subs: sub-0"), "{report}");
    assert!(report.contains("caveat subs: sub-1"), "{report}");
    assert!(report.contains("refused"), "{report}");
}

fn merged_caveat_campaign_with_rollup() -> (
    deadreckon_core::campaign::Campaign,
    deadreckon_core::campaign::CampaignRollup,
) {
    use deadreckon_core::campaign::build_rollup;
    let mut campaign = fixture_campaign();
    for (index, sub) in campaign.sub_goals.iter_mut().enumerate() {
        sub.result_run_id = Some(format!("run-{index}"));
        sub.sub_plan_id = Some(format!("plan-{index}"));
        sub.status = deadreckon_core::campaign::SubGoalStatus::Merged;
    }
    let rollup = build_rollup(&campaign, |_run_id| {
        (
            "signed".to_string(),
            deadreckon_core::tamper::AcceptanceTamperVerdict::Caveat,
            vec!["agent modified tests/x.rs".to_string()],
        )
    });
    (campaign, rollup)
}

#[test]
fn campaign_cross_sub_conflict_recommends_campaign_repair_once() {
    // The true conflict shape: every sub merged, the roll-up carries caveats.
    // (A refused roll-up means unmerged subs, where repair is guaranteed to
    // refuse — that state recommends resuming the interrupted children, pinned
    // in failure_surfacing_tests.)
    let (mut campaign, rollup) = merged_caveat_campaign_with_rollup();
    campaign.status = deadreckon_core::campaign::CampaignStatus::Failed;

    let surface = campaign_verdict_surface(None, &campaign, Some(&rollup));
    let rendered = surface.render_plain(false);

    assert!(rendered.starts_with("blocked campaign "), "{rendered}");
    assert!(
        rendered.contains("deterministic campaign-level refusal"),
        "{rendered}"
    );
    assert_eq!(
        rendered
            .matches(&format!(
                "deadreckon campaign repair {}",
                &campaign.campaign_id[..8]
            ))
            .count(),
        1,
        "{rendered}"
    );
    assert_eq!(
        surface.primary_action.command,
        format!("deadreckon campaign repair {}", &campaign.campaign_id[..8])
    );

    let summary = campaign_attach_summary(None, &campaign, Some(&rollup));
    assert!(summary.starts_with("blocked campaign "), "{summary}");
    assert!(summary.contains("Explanation"), "{summary}");
    assert!(summary.contains("Recommended"), "{summary}");
    assert_eq!(
        summary
            .matches(&format!(
                "deadreckon campaign repair {}",
                &campaign.campaign_id[..8]
            ))
            .count(),
        1,
        "{summary}"
    );
}

#[test]
fn no_hints_suppresses_campaign_completion_hints() {
    let (campaign, rollup) = campaign_with_rollup();
    let surface = campaign_verdict_surface(None, &campaign, Some(&rollup));
    assert!(
        !surface.secondary_actions.is_empty(),
        "campaign surface should carry secondary hints to suppress"
    );
    let shown = surface.render_plain(false);
    let hidden = surface.render_plain(true);
    assert!(shown.contains("\nSecondary\n"), "{shown}");
    assert!(
        !hidden.contains("\nSecondary\n"),
        "no_hints must drop the campaign completion secondary actions\n{hidden}"
    );
}

#[test]
fn campaign_completed_surface_recommends_apply_or_finish_once() {
    let (mut campaign, rollup) = campaign_with_rollup();
    campaign.status = deadreckon_core::campaign::CampaignStatus::Merged;
    campaign.merged_run_id = Some("d01795896e854713a51211cb7491f716".to_string());

    let surface = campaign_verdict_surface(None, &campaign, Some(&rollup));
    let rendered = surface.render_plain(false);

    assert!(rendered.starts_with("completed campaign "), "{rendered}");
    assert_eq!(
        rendered.matches("deadreckon apply d0179589").count(),
        1,
        "{rendered}"
    );
    assert_eq!(surface.primary_action.command, "deadreckon apply d0179589");
}

#[test]
fn campaign_json_primary_action_matches_human_primary_action() {
    let (mut campaign, rollup) = campaign_with_rollup();
    campaign.status = deadreckon_core::campaign::CampaignStatus::Failed;

    let surface = campaign_verdict_surface(None, &campaign, Some(&rollup));
    let value = surface.add_to_json(serde_json::json!({
        "kind": "campaign",
        "id": &campaign.campaign_id,
        "status": campaign_status_text(campaign.status),
        "next_actions": [surface.primary_action.command.clone()],
    }));

    assert_eq!(value["primary_action"], surface.primary_action.command);
    assert_eq!(
        value["verdict"]["recommended_command"],
        value["primary_action"]
    );
    assert_eq!(value["next_actions"][0], value["primary_action"]);
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
