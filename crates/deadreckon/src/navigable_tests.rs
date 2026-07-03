use std::time::Duration;

use super::commands::attach::{attach_should_return_to_plan, simulate_campaign_drill_cycle};
use super::commands::attach_runtime::{
    AttachSurface, AttachTickBudget, AttachTickTiming, CampaignAttachKeyAction,
    handle_campaign_key, note_tui_resumed, note_tui_suspended, reset_tui_suspend_depth,
    tui_suspend_depth,
};
use super::commands::campaign::{
    CampaignEventFeed, CampaignFeedEvent, campaign_attach_json_text,
    campaign_attach_state_from_dir, resolve_campaign,
};
use super::tui::{
    AttachCampaignParent, AttachParentPlan, AttachTuiState, PlanAttachRenderState, render_attach,
    render_campaign_attach_text, render_plan_attach,
};
use super::{AttachLive, AttachViewMode, NarrativeVisualMode};
use chrono::{Duration as ChronoDuration, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use deadreckon_core::campaign::{self, Campaign, CampaignStatus, RollupVerdict, SubGoalStatus};
use deadreckon_core::{
    DeadreckonPaths, Plan, PlanEventKind, PlanMode, PlanProviders, PlanRole, PlanStatus, PlanTask,
    PlanTaskStatus, RunOptions, RunStatus, append_plan_event, create_run, save_plan, save_state,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tempfile::TempDir;

fn fixture_paths() -> (TempDir, DeadreckonPaths) {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    (temp, paths)
}

fn fixture_plan(paths: &DeadreckonPaths, goal: &str) -> Plan {
    let mut task = PlanTask::new(
        0,
        format!("{goal} child"),
        format!("implement {goal} child"),
        PlanRole::Child,
        Some("smoke:child".to_string()),
    );
    task.status = PlanTaskStatus::Running;
    let second = PlanTask::new(
        1,
        format!("{goal} verify"),
        format!("verify {goal} child"),
        PlanRole::Child,
        Some("smoke:child".to_string()),
    );
    let mut plan = Plan::new(
        goal.to_string(),
        PlanMode::FullPlan,
        vec![task, second],
        PlanProviders::default(),
        Some("scope".to_string()),
        "0.1.0",
    )
    .expect("plan");
    plan.status = PlanStatus::Forked;
    save_plan(paths, &plan).expect("save plan");
    plan
}

fn fixture_campaign(paths: &DeadreckonPaths) -> (Campaign, Plan, Plan) {
    let plan_a = fixture_plan(paths, "alpha sub-plan");
    let plan_b = fixture_plan(paths, "beta sub-plan");
    let mut subs = campaign::build_sub_goals(
        vec!["alpha sub-plan".to_string(), "beta sub-plan".to_string()],
        2,
    )
    .expect("subs");
    subs[0].sub_plan_id = Some(plan_a.plan_id.clone());
    subs[0].status = SubGoalStatus::Running;
    subs[1].sub_plan_id = Some(plan_b.plan_id.clone());
    subs[1].status = SubGoalStatus::Pending;
    let mut campaign = Campaign::new(
        "ship navigable attach",
        subs,
        PlanProviders::default(),
        0,
        Some(12.0),
        None,
        "0.1.0",
    )
    .expect("campaign");
    campaign.campaign_id = "campnav000000000000000000000001".to_string();
    campaign.status = CampaignStatus::Forked;
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    campaign::write_campaign(&campaign_dir, &campaign).expect("write campaign");
    (campaign, plan_a, plan_b)
}

fn write_result_run(
    temp: &TempDir,
    paths: &DeadreckonPaths,
    campaign: &mut Campaign,
    sub_index: usize,
    spend: f64,
) {
    let mut state = create_run(
        paths,
        RunOptions {
            goal: format!("result {}", campaign.sub_goals[sub_index].sub_id),
            cwd: temp.path().to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some("smoke:child".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(20.0),
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");
    state.status = RunStatus::Completed;
    state.total_spend_usd = spend;
    save_state(&state).expect("save state");
    campaign.sub_goals[sub_index].result_run_id = Some(state.run_id);
}

#[tokio::test]
async fn campaign_feed_discovers_all_sub_plans_with_plan_ids() {
    let (_temp, paths) = fixture_paths();
    let (campaign, plan_a, plan_b) = fixture_campaign(&paths);
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);

    let mut feed = CampaignEventFeed::new(paths.clone(), campaign_dir, campaign.campaign_id);
    let events = feed.refresh(Duration::ZERO).await;

    assert!(events.iter().any(|event| matches!(
        event,
        CampaignFeedEvent::Snapshot { campaign } if campaign.sub_goals.len() == 2
    )));
    assert_eq!(
        feed.sub_plan_ids(),
        vec![plan_a.plan_id.clone(), plan_b.plan_id.clone()]
    );
}

#[tokio::test]
async fn campaign_feed_emits_campaign_and_sub_plan_events_deduped() {
    let (_temp, paths) = fixture_paths();
    let (campaign, plan_a, _plan_b) = fixture_campaign(&paths);
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    campaign::append_campaign_event(
        &campaign_dir,
        "campaign_started",
        serde_json::json!({ "n": 2 }),
    )
    .expect("campaign event");
    append_plan_event(
        &paths,
        &plan_a.plan_id,
        PlanEventKind::TaskReady {
            task_id: "task-0".to_string(),
            task_index: 0,
        },
    )
    .expect("plan event");
    let mut feed = CampaignEventFeed::new(
        paths.clone(),
        campaign_dir.clone(),
        campaign.campaign_id.clone(),
    );

    let first = feed.refresh(Duration::ZERO).await;
    let second = feed.refresh(Duration::ZERO).await;

    assert!(first.iter().any(|event| matches!(
        event,
        CampaignFeedEvent::Campaign { event } if event.kind == "campaign_started"
    )));
    assert!(first.iter().any(|event| matches!(
        event,
        CampaignFeedEvent::SubPlan { sub_id, event }
            if sub_id == "sub-0" && matches!(event.event, PlanEventKind::TaskReady { .. })
    )));
    assert!(
        second.iter().all(|event| !matches!(
            event,
            CampaignFeedEvent::Campaign { .. } | CampaignFeedEvent::SubPlan { .. }
        )),
        "{second:#?}"
    );
}

#[tokio::test]
async fn campaign_feed_tolerates_absent_event_files() {
    let (_temp, paths) = fixture_paths();
    let (campaign, _plan_a, _plan_b) = fixture_campaign(&paths);
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    let _ = std::fs::remove_file(campaign::campaign_events_path(&campaign_dir));
    let _ = std::fs::remove_file(
        paths.plan_events(&campaign.sub_goals[0].sub_plan_id.clone().unwrap()),
    );

    let mut feed = CampaignEventFeed::new(paths.clone(), campaign_dir, campaign.campaign_id);
    let events = feed.refresh(Duration::ZERO).await;

    assert!(
        events
            .iter()
            .any(|event| matches!(event, CampaignFeedEvent::Snapshot { .. }))
    );
}

#[test]
fn campaign_state_seeds_from_campaign_and_rollup() {
    let (_temp, paths) = fixture_paths();
    let (campaign, _plan_a, _plan_b) = fixture_campaign(&paths);
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    let rollup = campaign::CampaignRollup {
        schema_version: 1,
        campaign_id: campaign.campaign_id.clone(),
        evaluated_at: Utc::now(),
        leaves: Vec::new(),
        rollup_verdict: RollupVerdict::Clean,
        refused_subs: Vec::new(),
        caveat_subs: Vec::new(),
    };
    campaign::write_campaign_rollup(&campaign_dir, &rollup).expect("rollup");

    let state = campaign_attach_state_from_dir(&paths, &campaign_dir).expect("state");

    assert_eq!(state.campaign.campaign_id, campaign.campaign_id);
    assert_eq!(
        state.rollup.as_ref().map(|rollup| rollup.rollup_verdict),
        Some(RollupVerdict::Clean)
    );
    assert_eq!(state.selected, 0);
}

#[test]
fn campaign_aggregate_spend_sums_sub_result_runs() {
    let (temp, paths) = fixture_paths();
    let (mut campaign, _plan_a, _plan_b) = fixture_campaign(&paths);
    write_result_run(&temp, &paths, &mut campaign, 0, 1.25);
    write_result_run(&temp, &paths, &mut campaign, 1, 2.75);
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    campaign::write_campaign(&campaign_dir, &campaign).expect("campaign");

    let state = campaign_attach_state_from_dir(&paths, &campaign_dir).expect("state");

    assert!((state.aggregate_spend_usd - 4.0).abs() < f64::EPSILON);
}

#[test]
fn render_campaign_attach_shows_header_subs_rollup_budget() {
    let (temp, paths) = fixture_paths();
    let (mut campaign, _plan_a, _plan_b) = fixture_campaign(&paths);
    write_result_run(&temp, &paths, &mut campaign, 0, 1.50);
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    campaign::write_campaign(&campaign_dir, &campaign).expect("campaign");
    let mut state = campaign_attach_state_from_dir(&paths, &campaign_dir).expect("state");
    state.feed.push_back(CampaignFeedEvent::Campaign {
        event: campaign::CampaignEvent {
            schema_version: 1,
            ts: Utc::now(),
            kind: "campaign_started".to_string(),
            detail: serde_json::json!({ "n": 2 }),
        },
    });

    let text = render_campaign_attach_text(&state, false);

    assert!(text.contains("ship navigable attach"), "{text}");
    assert!(text.contains("tree budget $1.50 / $12.00"), "{text}");
    assert!(text.contains("sub-0 running"), "{text}");
    assert!(text.contains("sub-1 pending"), "{text}");
    assert!(text.contains("campaign_started"), "{text}");
}

#[test]
fn render_campaign_attach_marks_selected_sub() {
    let (_temp, paths) = fixture_paths();
    let (campaign, _plan_a, _plan_b) = fixture_campaign(&paths);
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    let mut state = campaign_attach_state_from_dir(&paths, &campaign_dir).expect("state");
    state.selected = 1;

    let text = render_campaign_attach_text(&state, false);

    assert!(text.contains("> sub-1 pending"), "{text}");
    assert!(text.contains("sub-0 running"), "{text}");
}

#[test]
fn campaign_footer_shows_keybindings_on_tty() {
    let (_temp, paths) = fixture_paths();
    let (campaign, _plan_a, _plan_b) = fixture_campaign(&paths);
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    let state = campaign_attach_state_from_dir(&paths, &campaign_dir).expect("state");

    let text = render_campaign_attach_text(&state, false);

    assert!(text.contains("Enter sub-plan"), "{text}");
    assert!(text.contains("b/Backspace"), "{text}");
    assert!(!text.contains("deadreckon attach <sub-plan-id>"), "{text}");
}

#[test]
fn campaign_keys_move_selection_clamped() {
    let (_temp, paths) = fixture_paths();
    let (campaign, _plan_a, _plan_b) = fixture_campaign(&paths);
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    let mut state = campaign_attach_state_from_dir(&paths, &campaign_dir).expect("state");

    assert_eq!(
        handle_campaign_key(&mut state, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
        CampaignAttachKeyAction::None
    );
    assert_eq!(state.selected, 0);
    handle_campaign_key(&mut state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(state.selected, 1);
    handle_campaign_key(&mut state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(state.selected, 1);
}

#[test]
fn campaign_enter_yields_drill_into_selected_sub() {
    let (_temp, paths) = fixture_paths();
    let (campaign, _plan_a, plan_b) = fixture_campaign(&paths);
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    let mut state = campaign_attach_state_from_dir(&paths, &campaign_dir).expect("state");
    state.selected = 1;

    let action = handle_campaign_key(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );

    assert_eq!(
        action,
        CampaignAttachKeyAction::DrillInto {
            sub_id: "sub-1".to_string(),
            plan_id: plan_b.plan_id,
        }
    );
}

#[test]
fn attach_campaign_tty_path_constructs_campaign_state() {
    let (_temp, paths) = fixture_paths();
    let (campaign, _plan_a, _plan_b) = fixture_campaign(&paths);
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);

    let state = campaign_attach_state_from_dir(&paths, &campaign_dir).expect("state");

    assert_eq!(state.campaign.campaign_id, campaign.campaign_id);
    assert_eq!(state.campaign.sub_goals.len(), 2);
}

#[test]
fn campaign_drill_into_sub_then_child_then_back_back_returns_to_campaign() {
    let (_temp, paths) = fixture_paths();
    let (campaign, plan_a, _plan_b) = fixture_campaign(&paths);
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    let mut state = campaign_attach_state_from_dir(&paths, &campaign_dir).expect("state");
    let action = handle_campaign_key(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert!(matches!(action, CampaignAttachKeyAction::DrillInto { .. }));
    assert!(attach_should_return_to_plan(KeyEvent::new(
        KeyCode::Char('b'),
        KeyModifiers::NONE
    )));

    let parent = AttachParentPlan {
        plan_id: plan_a.plan_id,
        task_id: "task-0".to_string(),
        campaign_parent: Some(AttachCampaignParent {
            campaign_id: campaign.campaign_id.clone(),
            sub_id: "sub-0".to_string(),
        }),
    };
    assert_eq!(parent.campaign_parent.as_ref().unwrap().sub_id, "sub-0");

    simulate_campaign_drill_cycle();
    assert_eq!(state.selected, 0);
}

#[test]
fn suspend_resume_depth_returns_to_zero_after_nested_drill() {
    reset_tui_suspend_depth();

    assert_eq!(note_tui_suspended(), 1);
    assert_eq!(note_tui_suspended(), 2);
    assert_eq!(note_tui_resumed(), 1);
    assert_eq!(note_tui_resumed(), 0);
    assert_eq!(tui_suspend_depth(), 0);
}

#[test]
fn plan_drilled_from_campaign_shows_campaign_breadcrumb() {
    let (_temp, paths) = fixture_paths();
    let (campaign, plan_a, _plan_b) = fixture_campaign(&paths);
    let parent = AttachCampaignParent {
        campaign_id: campaign.campaign_id,
        sub_id: "sub-0".to_string(),
    };

    let text = render_plan_attach_text_with_campaign_for_test(&paths, &plan_a, Some(parent));

    assert!(text.contains("campaign campnav0 / sub-0"), "{text}");
}

#[test]
fn run_breadcrumb_shows_full_campaign_chain() {
    let (temp, paths) = fixture_paths();
    let (campaign, plan_a, _plan_b) = fixture_campaign(&paths);
    let state = create_run(
        &paths,
        RunOptions {
            goal: "child detail".to_string(),
            cwd: temp.path().to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some("smoke:child".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");

    let text = render_attach_text_with_parent_for_test(
        &state,
        AttachParentPlan {
            plan_id: plan_a.plan_id,
            task_id: "task-0".to_string(),
            campaign_parent: Some(AttachCampaignParent {
                campaign_id: campaign.campaign_id,
                sub_id: "sub-0".to_string(),
            }),
        },
    );

    assert!(text.contains("campaign campnav0 / sub-0 / plan"), "{text}");
    assert!(
        text.contains("parent campaign campnav0 / sub-0 / plan"),
        "{text}"
    );
}

#[test]
fn attach_campaign_json_has_subs_rollup_and_budget() {
    let (_temp, paths) = fixture_paths();
    let (campaign, _plan_a, _plan_b) = fixture_campaign(&paths);
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    let state = campaign_attach_state_from_dir(&paths, &campaign_dir).expect("state");

    let text = campaign_attach_json_text(None, &state).expect("json");
    let value: serde_json::Value = serde_json::from_str(&text).expect("json value");

    assert_eq!(value["kind"], "campaign");
    assert_eq!(value["id"], campaign.campaign_id);
    assert_eq!(value["tree_budget_usd"], 12.0);
    assert_eq!(value["subs"].as_array().unwrap().len(), 2);
}

#[test]
fn campaign_summary_footer_keeps_drill_hint_in_plain_only() {
    let (_temp, paths) = fixture_paths();
    let (campaign, _plan_a, _plan_b) = fixture_campaign(&paths);
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    let state = campaign_attach_state_from_dir(&paths, &campaign_dir).expect("state");

    let tui = render_campaign_attach_text(&state, false);
    let plain = super::commands::campaign::campaign_attach_summary(None, &campaign, None);

    assert!(!tui.contains("deadreckon attach <sub-plan-id>"), "{tui}");
    assert!(plain.contains("deadreckon attach <sub-plan-id>"), "{plain}");
}

#[test]
fn campaign_header_shows_aggregate_spend_against_tree_budget() {
    let (temp, paths) = fixture_paths();
    let (mut campaign, _plan_a, _plan_b) = fixture_campaign(&paths);
    write_result_run(&temp, &paths, &mut campaign, 0, 3.25);
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    campaign::write_campaign(&campaign_dir, &campaign).expect("campaign");
    let state = campaign_attach_state_from_dir(&paths, &campaign_dir).expect("state");

    let text = render_campaign_attach_text(&state, false);

    assert!(text.contains("tree budget $3.25 / $12.00"), "{text}");
}

#[test]
fn tui_footer_omits_retype_hint_present_in_plain_summary() {
    let (_temp, paths) = fixture_paths();
    let (campaign, _plan_a, _plan_b) = fixture_campaign(&paths);
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    let state = campaign_attach_state_from_dir(&paths, &campaign_dir).expect("state");

    let text = render_campaign_attach_text(&state, false);
    assert!(text.contains("r refresh"), "{text}");
    assert!(!text.contains("attach <sub-plan-id>"), "{text}");
}

#[test]
fn campaign_keybindings_match_plan_attach_set() {
    let (_temp, paths) = fixture_paths();
    let (campaign, _plan_a, _plan_b) = fixture_campaign(&paths);
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    let mut state = campaign_attach_state_from_dir(&paths, &campaign_dir).expect("state");

    assert_eq!(
        handle_campaign_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)
        ),
        CampaignAttachKeyAction::Refresh
    );
    assert_eq!(
        handle_campaign_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE)
        ),
        CampaignAttachKeyAction::Back
    );
    assert_eq!(
        handle_campaign_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)
        ),
        CampaignAttachKeyAction::Quit
    );
}

#[test]
fn attach_campaign_latest_resolves_most_recent_campaign() {
    let (_temp, paths) = fixture_paths();
    let (mut old, _plan_a, _plan_b) = fixture_campaign(&paths);
    old.campaign_id = "campold000000000000000000000001".to_string();
    old.created_at = Utc::now() - ChronoDuration::minutes(5);
    campaign::write_campaign(&paths.plan_dir(&old.campaign_id), &old).expect("old");
    let (mut new, _plan_c, _plan_d) = fixture_campaign(&paths);
    new.campaign_id = "campnew000000000000000000000001".to_string();
    new.created_at = Utc::now();
    campaign::write_campaign(&paths.plan_dir(&new.campaign_id), &new).expect("new");

    let resolved = resolve_campaign(&paths, "latest")
        .expect("resolve")
        .expect("campaign")
        .1;

    assert_eq!(resolved.campaign_id, new.campaign_id);
}

#[test]
fn campaign_surface_has_tick_timing() {
    let mut tick = AttachTickTiming::new(AttachSurface::Campaign, AttachTickBudget::default());
    tick.record(
        super::commands::attach_runtime::AttachLoopStage::EventFeed,
        Duration::from_millis(1),
    );

    assert!(!tick.frame_exceeded());
}

fn render_plan_attach_text_with_campaign_for_test(
    paths: &DeadreckonPaths,
    plan: &Plan,
    campaign_parent: Option<AttachCampaignParent>,
) -> String {
    let backend = TestBackend::new(140, 34);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            render_plan_attach(
                frame,
                paths,
                plan,
                &PlanAttachRenderState {
                    messages: &[],
                    plan_events: &[],
                    feed_events: &[],
                    selected: 0,
                    selected_node: None,
                    zoomed_node: None,
                    show_hints: true,
                    view: AttachViewMode::Activity,
                    visual: NarrativeVisualMode::Architecture,
                    campaign_parent: campaign_parent.as_ref(),
                    narrative_notice: None,
                    narrative_projection: None,
                    narrative_scroll: 0,
                },
            )
        })
        .expect("draw");
    terminal_text(&terminal)
}

fn render_attach_text_with_parent_for_test(
    state: &deadreckon_core::PipelineState,
    parent_plan: AttachParentPlan,
) -> String {
    let backend = TestBackend::new(140, 34);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let tui_state = AttachTuiState {
        parent_plan: Some(parent_plan),
        ..AttachTuiState::default()
    };
    terminal
        .draw(|frame| {
            render_attach(
                frame,
                state,
                &[],
                &[],
                &[],
                &AttachLive::default(),
                &tui_state,
            )
        })
        .expect("draw");
    terminal_text(&terminal)
}

fn terminal_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let area = buffer.area;
    let mut text = String::new();
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            text.push_str(buffer.cell((x, y)).expect("cell").symbol());
        }
        text.push('\n');
    }
    text
}
