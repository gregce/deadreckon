use std::collections::VecDeque;
use std::io::Write;
use std::time::{Duration, Instant};

use super::commands::attach::{
    attach_should_quit, attach_should_return_to_plan, chain_narrative_refusal_text,
    run_narrative_json_text, run_narrative_plain_text,
};
use super::commands::attach_runtime::{
    AttachLoopStage, AttachNarrativeRefreshState, AttachPlanNarrativeRefreshJob,
    AttachRunNarrativeRefreshJob, AttachSurface, AttachTickBudget, AttachTickTiming,
    AttachWorkMode, PlanNarrativeRefreshInput, attach_loop_stage_work,
    cancel_plan_narrative_refresh_job, cancel_run_narrative_refresh_job,
    plan_narrative_refresh_request, poll_plan_narrative_refresh_job,
    poll_run_narrative_refresh_job, start_or_coalesce_plan_narrative_refresh_job,
};
use super::commands::campaign::{
    campaign_drop_subgoal_before_launch, campaign_edit_subgoal_before_launch,
    campaign_replace_sub_goals_before_launch,
};
use super::commands::chain::{
    chain_should_auto_attach, chain_step_dot, chain_wall_cap_hit, per_step_wall_cap,
};
use super::commands::doc::doc_polish_preview_text;
use super::commands::orchestrate::{recommend_child_count_for_goal, recommend_orchestration_mode};
use super::commands::plan::{
    implementation_plan_warnings, orchestration_dependency_rows, orchestration_parallelism_lines,
    orchestration_provider_role_rows, orchestration_role_table_lines,
};
use super::commands::start::{
    GoalShape, GoalShapeRecommendation, GoalShapeSource, StartDoneAction, StartDoneCriteriaSource,
    StartLaunchInput, StartPromptEligibility, StartPrompter, StartProviderSource,
    StartSelectedMode, StartSelectionSource, StartSourceMode, add_start_history_actions,
    apply_goal_shape_recommendation, fallback_goal_shape_recommendation, maybe_prompt_start_mode,
    parse_provider_goal_shape, prompt_start_done_criteria, prompt_start_existing_done_criteria,
    resolve_start_orchestration_options, start_done_materialization_request, start_launch_decision,
    start_launch_preview_facts, start_provider_role_summary,
};
use super::tui::{
    AttachActionNotice, AttachPanel, AttachPanelCounts, AttachPanelRows, AttachParentPlan,
    AttachTuiState, ChainAttachTuiState, build_run_narrative_projection, chain_activity_lines,
    chain_attach_footer_text, chain_attach_header_text, chain_event_read_hint,
    chain_timeline_lines, markdown_to_tui_lines, max_panel_scroll, render_chain_attach,
};
use super::{
    ATTACH_LIVE_FILE_DISPLAY_LIMIT, AcceptanceLive, AcceptanceUiStatus, AttachJsonlTail,
    AttachLive, AttachNarrativeProjectionCache, AttachProviderActivityCache,
    AttachProviderLogScanCache, AttachViewMode, COMMAND_HELP_CATALOG, CommandAudience,
    CommandDiscovery, CommandHelpEntry, CompletionAction, ConfigDefaults, HELP_ALL_GROUPS,
    LiveFile, NOUN_DONE_CONTRACT, NOUN_VERIFIED_RUN, NarrativeAcceptanceRefreshTracker,
    NarrativeQuietRefreshTracker, NarrativeRefreshKind, NarrativeVisualMode, PLAN_AS_BUILT,
    PLAN_CHILDREN, PLAN_DECISIONS, PLAN_DOC_PROVIDER_ERROR, PlanAttachRenderState,
    PlanDocRefreshOptions, PlanFeedEvent, PlanProviderAsBuilt, PlanProviderChild,
    PlanProviderDecisions, PlanProviderDocs, PlanProviderItem, PlanProviderNarrative,
    PlanWrapperDocContext, ProviderActivity, ProviderJsonlLogSpec, Result, RunNarrativeRenderInput,
    TopHelpGroup, acceptance_activity_lines, attach_banner, attach_header_text,
    attach_live_inventory, claude_project_name_for_workdir, cli_wait_status_line,
    collect_jsonl_provider_activity, collect_jsonl_provider_activity_scan, collect_plan_doc_input,
    command_discovery, completion_action_from_input, completion_hints_enabled,
    deadreckoning_course_ascii, deadreckoning_status_text, kill_banner, launch_preview_rows,
    live_file_lines, materialize_plan_docs_to_working, meter_color, narrative_provider_selection,
    plan_attach_footer, plan_doc_path, plan_merge_repair_summary_items,
    plan_narrative_refresh_trigger, provider_ingest_base_roots, provider_jsonl_activity_lines,
    provider_jsonl_log_spec_from_registry, provider_jsonl_session_matches_run,
    read_plan_events_lossy, refresh_plan_docs, render_attach, render_plan_attach,
    resolve_plan_doc_target, run_narrative_refresh_trigger, threshold_color,
    validate_plan_provider_docs, wrap_kv_value, write_plan_docs_deterministic,
    write_plan_docs_from_provider,
};
use crate::cli::{Cli, CliPlanMode, CliStartMode, StartCommandArgs};
use chrono::{Duration as ChronoDuration, Utc};
use clap::CommandFactory;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use deadreckon_core::flight::{FLIGHT_EVENTS_JSONL, FlightEvent, FlightEventKind, FlightUsage};
use deadreckon_core::{
    ApplyMode, ApplyStrategy, BranchPolicy, CapabilityPreview, Chain, ChainEvent, ChainEventKind,
    ChainNewOptions, ChainStatus, ChainStepStatus, DeadreckonPaths, DocKind, NetworkCapability,
    OnFail, Plan, PlanEvent, PlanEventKind, PlanMessage, PlanMessageKind, PlanMode, PlanProviders,
    PlanRole, PlanStatus, PlanTask, PlanTaskStatus, RunEvent, RunEventKind, RunListEntry,
    RunOptions, RunStatus, SpendRecord, TraceRecord, append_plan_event, append_trace, create_run,
    doc_path_for_kind, save_plan, write_child_summary, write_worker_spec,
};
use deadreckon_providers::SpendEstimate;
use deadreckon_providers::registry::{
    IngestCwdMatch, IngestDescriptor, IngestStorage, ProviderRegistry,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};
use ratatui::text::Line;
use tokio_util::sync::CancellationToken;

fn chain_fixture() -> Chain {
    let mut chain = Chain::new(ChainNewOptions {
        root_goal: "build app".to_string(),
        goals: vec!["first step".to_string(), "second step".to_string()],
        scope: "scope".to_string(),
        base_branch: "main".to_string(),
        base_sha: "abcdef123456".to_string(),
        cwd: std::path::PathBuf::from("/tmp/project"),
        provider: Some("smoke".to_string()),
        model: None,
        sandbox: "none".to_string(),
        branch_policy: BranchPolicy::Stack,
        apply_mode: ApplyMode::Auto,
        apply_strategy: ApplyStrategy::Squash,
        apply_allowlist: Vec::new(),
        on_fail: OnFail::Stop,
        circuit_breaker_threshold: 2,
        max_spend_usd: Some(5.0),
        max_wall_seconds: Some(600.0),
        deadreckon_version: "0.1.0".to_string(),
    })
    .expect("chain");
    chain.steps[0].status = ChainStepStatus::Applied;
    chain.steps[0].run_id = Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string());
    chain.steps[1].status = ChainStepStatus::Running;
    chain
}

fn chain_event_record(chain_id: &str, index: usize) -> ChainEvent {
    ChainEvent {
        timestamp: Utc::now(),
        chain_id: chain_id.to_string(),
        event: ChainEventKind::ChainStepStarted,
        step_index: Some(index as u32),
        detail: serde_json::json!({ "goal": format!("step {index}") }),
    }
}

#[test]
fn attach_tick_budget_records_slow_sync_stage_without_panicking() {
    let budget = AttachTickBudget {
        target_frame_ms: 100,
        max_sync_io_ms: 5,
        slow_warning_ms: 8,
    };
    let mut timing = AttachTickTiming::new(AttachSurface::Run, budget);

    timing.record(AttachLoopStage::LiveCollect, Duration::from_millis(9));

    let slow_sync = timing.slow_sync_stages();
    assert_eq!(slow_sync.len(), 1);
    assert_eq!(slow_sync[0].stage, AttachLoopStage::LiveCollect);
    assert_eq!(slow_sync[0].stage.label(), "live collect");
    assert_eq!(timing.slow_warning_stages().len(), 1);
    assert!(!timing.frame_exceeded());
}

#[test]
fn run_attach_loop_model_marks_provider_refresh_as_async_work() {
    assert_eq!(
        attach_loop_stage_work(
            AttachSurface::Run,
            AttachLoopStage::ProviderNarrativeRefresh
        ),
        AttachWorkMode::Background
    );
    assert_eq!(
        attach_loop_stage_work(AttachSurface::Run, AttachLoopStage::LiveCollect),
        AttachWorkMode::UiSync
    );
}

#[test]
fn plan_attach_loop_model_marks_provider_refresh_as_async_work() {
    assert_eq!(
        attach_loop_stage_work(
            AttachSurface::Plan,
            AttachLoopStage::ProviderNarrativeRefresh
        ),
        AttachWorkMode::Background
    );
    assert_eq!(
        attach_loop_stage_work(AttachSurface::Chain, AttachLoopStage::ReadJsonl),
        AttachWorkMode::UiSync
    );
}

#[test]
fn run_attach_manual_refresh_does_not_block_quit() {
    let token = CancellationToken::new();
    let refresh = AttachNarrativeRefreshState::new(NarrativeRefreshKind::Manual, Utc::now(), token);

    assert!(refresh.start_notice().contains("background"));
    assert!(attach_should_return_to_plan(KeyEvent::new(
        KeyCode::Char('q'),
        KeyModifiers::empty()
    )));
}

#[tokio::test]
async fn slow_run_narrator_still_allows_quit() {
    let token = CancellationToken::new();
    let handle = tokio::spawn(async { std::future::pending::<String>().await });
    let mut job = Some(AttachRunNarrativeRefreshJob {
        state: AttachNarrativeRefreshState::new(
            NarrativeRefreshKind::Manual,
            Utc::now(),
            token.clone(),
        ),
        handle,
    });

    let poll = tokio::time::timeout(
        Duration::from_millis(20),
        poll_run_narrative_refresh_job(&mut job),
    )
    .await
    .expect("poll should return without waiting for slow narrator");

    assert!(poll.is_none());
    assert!(attach_should_quit(KeyEvent::new(
        KeyCode::Char('q'),
        KeyModifiers::empty()
    )));
    assert!(attach_should_return_to_plan(KeyEvent::new(
        KeyCode::Backspace,
        KeyModifiers::empty()
    )));
    assert!(cancel_run_narrative_refresh_job(&mut job));
    assert!(token.is_cancelled());
}

#[test]
fn run_attach_manual_refresh_coalesces_when_in_flight() {
    let token = CancellationToken::new();
    let started_at = Utc::now();
    let mut refresh =
        AttachNarrativeRefreshState::new(NarrativeRefreshKind::Manual, started_at, token);

    let notice = refresh.coalesce(
        NarrativeRefreshKind::Event("run completed"),
        started_at + ChronoDuration::seconds(4),
    );

    assert_eq!(refresh.coalesced_requests, 1);
    assert!(notice.contains("already running"));
    assert!(notice.contains("coalesced run completed"));
}

#[test]
fn run_attach_refresh_completion_updates_notice_once() {
    let token = CancellationToken::new();
    let mut refresh =
        AttachNarrativeRefreshState::new(NarrativeRefreshKind::Manual, Utc::now(), token);

    assert_eq!(
        refresh.completion_notice_once("provider narrative refreshed".to_string()),
        Some("provider narrative refreshed".to_string())
    );
    assert_eq!(
        refresh.completion_notice_once("duplicate completion".to_string()),
        None
    );
}

#[test]
fn run_attach_detach_cancels_in_flight_refresh() {
    let token = CancellationToken::new();
    let refresh =
        AttachNarrativeRefreshState::new(NarrativeRefreshKind::Manual, Utc::now(), token.clone());

    refresh.cancel();

    assert!(token.is_cancelled());
}

#[test]
fn run_attach_event_refresh_spawns_background_job() {
    let token = CancellationToken::new();
    let refresh = AttachNarrativeRefreshState::new(
        NarrativeRefreshKind::Event("run completed"),
        Utc::now(),
        token,
    );

    let notice = refresh.start_notice();

    assert!(notice.contains("run completed"));
    assert!(notice.contains("background"));
}

#[test]
fn run_attach_quiet_refresh_does_not_block_frame_draw() {
    assert_eq!(
        attach_loop_stage_work(
            AttachSurface::Run,
            AttachLoopStage::ProviderNarrativeRefresh
        ),
        AttachWorkMode::Background
    );
    assert_eq!(
        NarrativeRefreshKind::QuietThreshold.label(),
        "quiet threshold"
    );
}

#[test]
fn run_attach_auto_refresh_skips_when_manual_refresh_in_flight() {
    let token = CancellationToken::new();
    let started_at = Utc::now();
    let mut refresh =
        AttachNarrativeRefreshState::new(NarrativeRefreshKind::Manual, started_at, token);

    let notice = refresh.coalesce(
        NarrativeRefreshKind::QuietThreshold,
        started_at + ChronoDuration::seconds(2),
    );

    assert_eq!(refresh.kind, NarrativeRefreshKind::Manual);
    assert_eq!(refresh.coalesced_requests, 1);
    assert!(notice.contains("coalesced quiet threshold"));
}

#[test]
fn run_attach_refresh_failure_remains_visible_until_replaced() {
    let counts = AttachPanelCounts {
        activity: 1,
        files: 0,
        processes: 0,
    };
    let rows = AttachPanelRows {
        activity: 3,
        files: 0,
        processes: 0,
    };
    let mut state = AttachTuiState::default();
    state.record_narrative_refresh("provider refresh failed: timeout".to_string());
    state.handle_key(
        KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()),
        counts,
        rows,
    );
    assert_eq!(
        state.narrative_notice.as_deref(),
        Some("provider refresh failed: timeout")
    );

    state.record_narrative_refresh("provider narrative refreshed".to_string());

    assert_eq!(
        state.narrative_notice.as_deref(),
        Some("provider narrative refreshed")
    );
}

#[test]
fn plan_attach_manual_refresh_does_not_block_quit() {
    let token = CancellationToken::new();
    let refresh = AttachNarrativeRefreshState::new(NarrativeRefreshKind::Manual, Utc::now(), token);

    let notice = refresh.start_notice();

    assert_eq!(
        attach_loop_stage_work(
            AttachSurface::Plan,
            AttachLoopStage::ProviderNarrativeRefresh
        ),
        AttachWorkMode::Background
    );
    assert!(notice.contains("background"));
    assert!(notice.contains("q detaches immediately"));
}

#[tokio::test]
async fn slow_plan_narrator_still_allows_visual_toggle() {
    let token = CancellationToken::new();
    let handle = tokio::spawn(async { std::future::pending::<String>().await });
    let mut job = Some(AttachPlanNarrativeRefreshJob {
        plan_id: "plan-slow".to_string(),
        state: AttachNarrativeRefreshState::new(
            NarrativeRefreshKind::Manual,
            Utc::now(),
            token.clone(),
        ),
        handle,
    });
    let visual = NarrativeVisualMode::Architecture;

    let poll = tokio::time::timeout(
        Duration::from_millis(20),
        poll_plan_narrative_refresh_job(&mut job),
    )
    .await
    .expect("poll should return without waiting for slow narrator");

    assert!(poll.is_none());
    assert_eq!(visual.next(), NarrativeVisualMode::Agents);
    assert!(cancel_plan_narrative_refresh_job(&mut job));
    assert!(token.is_cancelled());
}

#[test]
fn plan_attach_event_refresh_spawns_background_job() {
    let (_, _, plan) = full_plan_fixture(2);
    let event = PlanEvent {
        timestamp: Utc::now(),
        plan_id: plan.plan_id.clone(),
        event: PlanEventKind::TaskCompleted {
            task_id: "task-0".to_string(),
            task_index: 0,
            run_id: Some("run-1".to_string()),
            status: "completed".to_string(),
        },
    };
    let kind = plan_narrative_refresh_trigger(&[PlanFeedEvent::Plan { event }]).expect("trigger");
    let refresh = AttachNarrativeRefreshState::new(kind, Utc::now(), CancellationToken::new());

    let notice = refresh.start_notice();

    assert_eq!(kind, NarrativeRefreshKind::Event("plan child completed"));
    assert!(notice.contains("plan child completed"));
    assert!(notice.contains("background"));
}

#[tokio::test]
async fn plan_attach_refresh_coalesces_by_plan_id() {
    let (_, paths, plan) = full_plan_fixture(2);
    let started_at = Utc::now();
    let token = CancellationToken::new();
    let handle = tokio::spawn(async { "stale refresh".to_string() });
    let mut job = Some(AttachPlanNarrativeRefreshJob {
        plan_id: plan.plan_id.clone(),
        state: AttachNarrativeRefreshState::new(NarrativeRefreshKind::Manual, started_at, token),
        handle,
    });
    let narrative_config = Default::default();
    let refresh_input = PlanNarrativeRefreshInput {
        paths: &paths,
        plan: &plan,
        messages: &[],
        plan_events: &[],
        feed_events: &[],
        selected: 0,
        config: &narrative_config,
    };
    let request = plan_narrative_refresh_request(
        &refresh_input,
        NarrativeRefreshKind::Event("plan child completed"),
    );

    let notice = start_or_coalesce_plan_narrative_refresh_job(
        &mut job,
        request,
        started_at + ChronoDuration::seconds(3),
    );

    let active = job.as_ref().expect("job remains active");
    assert_eq!(active.plan_id, plan.plan_id);
    assert_eq!(active.state.kind, NarrativeRefreshKind::Manual);
    assert_eq!(active.state.coalesced_requests, 1);
    assert!(notice.contains("coalesced plan child completed"));
    assert!(cancel_plan_narrative_refresh_job(&mut job));
}

#[tokio::test]
async fn plan_attach_child_drill_cancels_or_suspends_refresh_cleanly() {
    let (_, _, plan) = full_plan_fixture(2);
    let token = CancellationToken::new();
    let handle = tokio::spawn(async { std::future::pending::<String>().await });
    let mut job = Some(AttachPlanNarrativeRefreshJob {
        plan_id: plan.plan_id,
        state: AttachNarrativeRefreshState::new(
            NarrativeRefreshKind::Event("plan child completed"),
            Utc::now(),
            token.clone(),
        ),
        handle,
    });

    assert!(cancel_plan_narrative_refresh_job(&mut job));
    assert!(job.is_none());
    assert!(token.is_cancelled());
}

#[test]
fn attach_live_inventory_prunes_node_modules_before_descending() {
    let temp = test_tempdir();
    let root = temp.path();
    std::fs::create_dir_all(root.join("src")).expect("src");
    std::fs::create_dir_all(root.join("node_modules/pkg/deep")).expect("node modules");
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("source");
    std::fs::write(root.join("node_modules/pkg/deep/index.js"), "ignored\n").expect("ignored");

    let inventory = attach_live_inventory(root);

    assert_eq!(inventory.file_count, 1);
    assert_eq!(inventory.files.len(), 1);
    assert_eq!(inventory.files[0].path, "src/main.rs");
}

#[test]
fn attach_live_inventory_prunes_chrome_profile_tmp_before_descending() {
    let temp = test_tempdir();
    let root = temp.path();
    std::fs::create_dir_all(root.join("app")).expect("app");
    std::fs::create_dir_all(root.join(".tmp/chrome-profile-123/Default")).expect("profile");
    std::fs::write(root.join("app/index.ts"), "export {}\n").expect("source");
    std::fs::write(
        root.join(".tmp/chrome-profile-123/Default/Cookies"),
        "sqlite-ish",
    )
    .expect("profile file");

    let inventory = attach_live_inventory(root);

    assert_eq!(inventory.file_count, 1);
    assert_eq!(inventory.files.len(), 1);
    assert_eq!(inventory.files[0].path, "app/index.ts");
}

#[test]
fn attach_live_inventory_still_counts_recent_project_files() {
    let temp = test_tempdir();
    let root = temp.path();
    std::fs::create_dir_all(root.join("src")).expect("src");
    std::fs::write(root.join("README.md"), "hello\n").expect("readme");
    std::fs::write(root.join("src/lib.rs"), "pub fn value() {}\n").expect("lib");

    let inventory = attach_live_inventory(root);
    let paths = inventory
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();

    assert_eq!(inventory.file_count, 2);
    assert_eq!(
        inventory.total_bytes,
        "hello\n".len() as u64 + "pub fn value() {}\n".len() as u64
    );
    assert!(paths.contains(&"README.md"));
    assert!(paths.contains(&"src/lib.rs"));
}

#[test]
fn attach_live_inventory_caps_display_without_losing_total_count() {
    let temp = test_tempdir();
    let root = temp.path();
    std::fs::create_dir_all(root.join("src")).expect("src");
    for index in 0..(ATTACH_LIVE_FILE_DISPLAY_LIMIT + 3) {
        std::fs::write(root.join("src").join(format!("file-{index:03}.txt")), "x").expect("file");
    }

    let inventory = attach_live_inventory(root);
    let live = AttachLive {
        file_count: inventory.file_count,
        total_bytes: inventory.total_bytes,
        files: inventory.files,
        working_dir_exists: true,
        ..AttachLive::default()
    };
    let lines = live_file_lines(&live);

    assert_eq!(live.file_count, ATTACH_LIVE_FILE_DISPLAY_LIMIT + 3);
    assert_eq!(live.files.len(), ATTACH_LIVE_FILE_DISPLAY_LIMIT);
    assert_eq!(
        live.total_bytes,
        (ATTACH_LIVE_FILE_DISPLAY_LIMIT + 3) as u64
    );
    assert!(
        lines
            .last()
            .is_some_and(|line| line == "... 3 more files not shown")
    );
}

#[test]
fn large_worktree_live_files_still_draws_recent_files() {
    let (_temp, state) = doc_preview_state();
    let root = &state.working_dir;
    std::fs::create_dir_all(root.join("bulk")).expect("bulk");
    for index in 0..(ATTACH_LIVE_FILE_DISPLAY_LIMIT + 80) {
        std::fs::write(
            root.join("bulk").join(format!("file-{index:03}.txt")),
            "old",
        )
        .expect("bulk file");
    }
    std::thread::sleep(Duration::from_millis(20));
    std::fs::create_dir_all(root.join("src")).expect("src");
    std::fs::write(root.join("src/recent-feature.rs"), "pub fn recent() {}\n")
        .expect("recent file");

    let inventory = attach_live_inventory(root);
    let live = AttachLive {
        file_count: inventory.file_count,
        total_bytes: inventory.total_bytes,
        files: inventory.files,
        working_dir_exists: true,
        ..AttachLive::default()
    };
    let text = render_attach_text_with_tui_state(
        &state,
        &[],
        &live,
        AttachTuiState {
            focused_panel: AttachPanel::Files,
            ..AttachTuiState::default()
        },
    );

    assert!(live.file_count > ATTACH_LIVE_FILE_DISPLAY_LIMIT);
    assert!(
        live_file_lines(&live)
            .iter()
            .any(|line| line.contains("src/recent-feature.rs"))
    );
    assert!(text.contains("src/recent-feature.rs"), "{text}");
}

#[test]
fn attach_jsonl_tail_reads_only_appended_rows() {
    let temp = test_tempdir();
    let path = temp.path().join("events.jsonl");
    std::fs::write(&path, "{\"n\":1}\n").expect("jsonl");
    let mut tail = AttachJsonlTail::<serde_json::Value>::new(path.clone());

    assert_eq!(tail.refresh().expect("first").len(), 1);
    append_jsonl_raw(&path, "{\"n\":2}");
    assert_eq!(tail.refresh().expect("second").len(), 2);
    assert_eq!(tail.refresh().expect("third").len(), 2);
}

#[test]
fn attach_jsonl_tail_tolerates_partial_last_line() {
    let temp = test_tempdir();
    let path = temp.path().join("events.jsonl");
    std::fs::write(&path, "{\"n\":1}\n{\"n\"").expect("partial");
    let mut tail = AttachJsonlTail::<serde_json::Value>::new(path.clone());

    assert_eq!(tail.refresh().expect("partial read").len(), 1);
    append_raw(&path, ":2}\n");
    let rows = tail.refresh().expect("completed partial");

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["n"], serde_json::json!(2));
}

#[test]
fn run_attach_spend_and_trace_cache_updates_from_mtime() {
    let temp = test_tempdir();
    let spend_path = temp.path().join("spend.jsonl");
    let trace_path = temp.path().join("traces.jsonl");
    let mut spend_tail = AttachJsonlTail::<SpendRecord>::new(spend_path.clone());
    let mut trace_tail = AttachJsonlTail::<TraceRecord>::new(trace_path.clone());
    append_jsonl_value(&spend_path, &spend_record(1));
    append_jsonl_value(&trace_path, &trace_record("run-1", 1));

    assert_eq!(spend_tail.refresh().expect("spend first").len(), 1);
    assert_eq!(trace_tail.refresh().expect("trace first").len(), 1);

    append_jsonl_value(&spend_path, &spend_record(2));
    append_jsonl_value(&trace_path, &trace_record("run-1", 2));

    assert_eq!(spend_tail.refresh().expect("spend second").len(), 2);
    assert_eq!(trace_tail.refresh().expect("trace second").len(), 2);
}

#[test]
fn run_attach_flight_activity_uses_incremental_rows() {
    let (_temp, state) = doc_preview_state();
    let path = state.run_root.join(FLIGHT_EVENTS_JSONL);
    append_jsonl_value(&path, &flight_event(&state.run_id, 1, None));
    let mut cache = AttachProviderActivityCache::new(&state);

    let activity = cache.refresh(&state);
    assert_eq!(activity.lines.len(), 1);
    assert!(activity.lines[0].contains("flight #000001"));

    append_jsonl_value(
        &path,
        &flight_event(
            &state.run_id,
            2,
            Some(FlightUsage {
                input_tokens: 10,
                output_tokens: 5,
                context_window: Some(100),
            }),
        ),
    );
    let activity = cache.refresh(&state);

    assert_eq!(activity.lines.len(), 2);
    assert!(activity.lines[1].contains("flight #000002"));
    assert_eq!(activity.context_tokens, Some(15));
    assert_eq!(activity.context_window, Some(100));
    assert_eq!(cache.refresh(&state).lines.len(), 2);
}

#[test]
fn chain_attach_uses_incremental_event_tail() {
    let temp = test_tempdir();
    let path = temp.path().join("chain-events.jsonl");
    append_jsonl_value(&path, &chain_event_record("chain-test", 0));
    let mut tail = AttachJsonlTail::<ChainEvent>::new(path.clone());

    assert_eq!(tail.refresh().expect("initial").len(), 1);
    append_jsonl_value(&path, &chain_event_record("chain-test", 1));
    let full_len = std::fs::metadata(&path).expect("metadata").len() as usize;
    let row_count = tail.refresh().expect("appended").len();

    assert_eq!(row_count, 2);
    assert_eq!(tail.refresh_count, 2);
    assert_eq!(tail.last_appended_rows, 1);
    assert!(tail.last_read_bytes < full_len);
}

#[test]
fn chain_attach_large_event_file_keeps_tick_under_budget() {
    let temp = test_tempdir();
    let path = temp.path().join("chain-events.jsonl");
    for index in 0..2_000 {
        append_jsonl_value(&path, &chain_event_record("chain-large", index));
    }
    let mut tail = AttachJsonlTail::<ChainEvent>::new(path.clone());
    assert_eq!(tail.refresh().expect("initial").len(), 2_000);
    append_jsonl_value(&path, &chain_event_record("chain-large", 2_000));

    let started = Instant::now();
    let row_count = tail.refresh().expect("incremental").len();
    let elapsed = started.elapsed();
    let hint = chain_event_read_hint(
        row_count,
        tail.last_appended_rows,
        tail.partial_bytes,
        elapsed,
        AttachTickBudget::default(),
        None,
    );

    assert_eq!(row_count, 2_001);
    assert_eq!(tail.last_appended_rows, 1);
    assert!(
        tail.last_read_bytes < 512,
        "expected appended-only read, read {} bytes",
        tail.last_read_bytes
    );
    assert!(
        elapsed < Duration::from_millis(AttachTickBudget::default().max_sync_io_ms),
        "incremental chain event refresh took {elapsed:?}"
    );
    assert!(hint.is_none(), "unexpected slow-read hint: {hint:?}");
}

#[test]
fn chain_attach_partial_event_line_is_ignored_until_complete() {
    let temp = test_tempdir();
    let path = temp.path().join("chain-events.jsonl");
    let first = serde_json::to_string(&chain_event_record("chain-partial", 0)).expect("first");
    let second = serde_json::to_string(&chain_event_record("chain-partial", 1)).expect("second");
    let split = second.len() / 2;
    std::fs::write(&path, format!("{first}\n{}", &second[..split])).expect("partial");
    let mut tail = AttachJsonlTail::<ChainEvent>::new(path.clone());

    assert_eq!(tail.refresh().expect("partial").len(), 1);
    assert!(tail.partial_bytes > 0);
    append_raw(&path, &format!("{}\n", &second[split..]));
    let row_count = tail.refresh().expect("completed").len();

    assert_eq!(row_count, 2);
    assert_eq!(tail.last_appended_rows, 1);
    assert_eq!(tail.partial_bytes, 0);
}

#[test]
fn provider_activity_does_not_rescan_roots_each_tick() {
    let (_run_temp, state) = doc_preview_state();
    let temp = test_tempdir();
    let root = temp.path();
    let log = root.join("session.jsonl");
    write_codex_provider_log(&log, &state.working_dir, "first");
    let spec = provider_log_spec(root, &state);
    let mut cache = AttachProviderLogScanCache::default();
    let now = Instant::now();

    let first = cache.refresh(&state, &spec, false, now);
    let second = cache.refresh(&state, &spec, false, now + Duration::from_secs(1));

    assert_eq!(cache.root_scan_count, 1);
    assert_eq!(first.lines, second.lines);
    assert!(first.lines.iter().any(|line| line.contains("agent first")));
}

#[test]
fn provider_activity_prefers_flight_rows_over_fallback_scan() {
    let (_run_temp, state) = doc_preview_state();
    let temp = test_tempdir();
    let root = temp.path();
    write_codex_provider_log(&root.join("session.jsonl"), &state.working_dir, "fallback");
    let spec = provider_log_spec(root, &state);
    let mut cache = AttachProviderLogScanCache::default();

    let activity = cache.refresh(&state, &spec, true, Instant::now());

    assert_eq!(cache.root_scan_count, 0);
    assert!(activity.lines.is_empty());
}

#[test]
fn provider_activity_fallback_scan_respects_freshness_and_cwd() {
    let (_run_temp, state) = doc_preview_state();
    let temp = test_tempdir();
    let root = temp.path();
    write_codex_provider_log(
        &root.join("wrong.jsonl"),
        std::path::Path::new("/elsewhere"),
        "wrong",
    );
    write_codex_provider_log(&root.join("right.jsonl"), &state.working_dir, "right");
    let spec = provider_log_spec(root, &state);

    let scan = collect_jsonl_provider_activity_scan(&state, &spec);

    assert!(
        scan.matched_path
            .as_ref()
            .is_some_and(|path| path.ends_with("right.jsonl"))
    );
    assert!(
        scan.activity
            .lines
            .iter()
            .any(|line| line.contains("agent right"))
    );
    assert!(
        !scan
            .activity
            .lines
            .iter()
            .any(|line| line.contains("agent wrong"))
    );
}

#[test]
fn provider_activity_cache_invalidates_when_matching_log_changes() {
    let (_run_temp, state) = doc_preview_state();
    let temp = test_tempdir();
    let root = temp.path();
    let log = root.join("session.jsonl");
    write_codex_provider_log(&log, &state.working_dir, "first");
    let spec = provider_log_spec(root, &state);
    let mut cache = AttachProviderLogScanCache::default();
    let now = Instant::now();

    let first = cache.refresh(&state, &spec, false, now);
    std::thread::sleep(Duration::from_millis(20));
    append_codex_agent_message(&log, "second");
    let second = cache.refresh(&state, &spec, false, now + Duration::from_secs(1));

    assert_eq!(cache.root_scan_count, 2);
    assert!(first.lines.iter().any(|line| line.contains("agent first")));
    assert!(
        second
            .lines
            .iter()
            .any(|line| line.contains("agent second"))
    );
}

#[test]
fn run_narrative_render_uses_cached_projection_when_coverage_unchanged() {
    let (_temp, state) = doc_preview_state();
    let live = AttachLive {
        working_dir_exists: true,
        ..AttachLive::default()
    };
    let tui_state = AttachTuiState {
        view: AttachViewMode::Narrative,
        ..AttachTuiState::default()
    };
    let input = RunNarrativeRenderInput {
        state: &state,
        spend: &[],
        traces: &[],
        events: &[],
        live: &live,
        tui_state: &tui_state,
    };
    let mut cache = AttachNarrativeProjectionCache::default();

    let first = cache.refresh_run(&input).expect("first projection");
    let second = cache.refresh_run(&input).expect("cached projection");

    assert_eq!(cache.refresh_count, 1);
    assert_eq!(first.snapshot.snapshot_id, second.snapshot.snapshot_id);
}

#[test]
fn plan_narrative_render_uses_cached_projection_when_feed_unchanged() {
    let (_temp, paths, plan) = full_plan_fixture(2);
    let input = crate::narrative::PlanNarrativeInput {
        paths: &paths,
        plan: &plan,
        messages: &[],
        plan_events: &[],
        feed_events: &[],
        selected: 0,
    };
    let mut cache = AttachNarrativeProjectionCache::default();

    let first = cache.refresh_plan(&input).expect("first projection");
    let second = cache.refresh_plan(&input).expect("cached projection");

    assert_eq!(cache.refresh_count, 1);
    assert_eq!(first.snapshot.snapshot_id, second.snapshot.snapshot_id);
}

#[test]
fn render_attach_frame_unit_snapshot() {
    let (_temp, state) = doc_preview_state();
    let live = AttachLive {
        working_dir_exists: true,
        ..AttachLive::default()
    };

    let text = render_attach_text_with_size(&state, &[], &live, AttachTuiState::default(), 100, 24);

    assert!(text.contains("deadreckon"), "{text}");
    assert!(text.contains("provider cli:codex"), "{text}");
    assert!(text.contains("goal preview docs"), "{text}");
    assert!(text.contains("tool calls / provider activity"), "{text}");
    assert!(text.contains("live files"), "{text}");
    assert!(text.contains("processes"), "{text}");
}

#[test]
fn render_attach_text_does_not_append_narrative_snapshots() {
    let (_temp, state) = doc_preview_state();
    let snapshots = state.run_root.join("narrative/snapshots.jsonl");
    let tui_state = AttachTuiState {
        view: AttachViewMode::Narrative,
        ..AttachTuiState::default()
    };

    let _ =
        render_attach_text_with_tui_state(&state, &[], &AttachLive::default(), tui_state.clone());
    let _ = render_attach_text_with_tui_state(&state, &[], &AttachLive::default(), tui_state);

    assert_eq!(jsonl_line_count(&snapshots), 0);
}

#[test]
fn stale_provider_snapshot_survives_redraw_without_churn() {
    let (_temp, state) = doc_preview_state();
    let live = AttachLive {
        working_dir_exists: true,
        ..AttachLive::default()
    };
    let tui_state = AttachTuiState {
        view: AttachViewMode::Narrative,
        ..AttachTuiState::default()
    };
    let input = RunNarrativeRenderInput {
        state: &state,
        spend: &[],
        traces: &[],
        events: &[],
        live: &live,
        tui_state: &tui_state,
    };
    let deterministic = build_run_narrative_projection(&input);
    let stale = crate::narrative::projection_with_provider_failure(
        &deterministic,
        Some("cli:test".to_string()),
        "timeout",
    );
    crate::narrative::persist_run_projection(&state, &stale).expect("persist stale");
    let snapshots = state.run_root.join("narrative/snapshots.jsonl");
    let before = jsonl_line_count(&snapshots);
    let mut cache = AttachNarrativeProjectionCache::default();

    let first = cache.refresh_run(&input).expect("first projection");
    let second = cache.refresh_run(&input).expect("cached projection");
    let after = jsonl_line_count(&snapshots);

    assert_eq!(cache.refresh_count, 1);
    assert_eq!(before, after);
    assert_eq!(
        first.state.latest_status,
        crate::narrative::NarrativeStatus::Stale
    );
    assert_eq!(
        second.state.latest_status,
        crate::narrative::NarrativeStatus::Stale
    );
}

fn jsonl_line_count(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path)
        .map(|raw| raw.lines().filter(|line| !line.trim().is_empty()).count())
        .unwrap_or(0)
}

fn append_jsonl_raw(path: &std::path::Path, raw: &str) {
    append_raw(path, &format!("{raw}\n"));
}

fn append_raw(path: &std::path::Path, raw: &str) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open jsonl");
    file.write_all(raw.as_bytes()).expect("write jsonl");
}

fn append_jsonl_value<T: serde::Serialize>(path: &std::path::Path, value: &T) {
    append_jsonl_raw(path, &serde_json::to_string(value).expect("json"));
}

fn spend_record(turn: u32) -> SpendRecord {
    SpendRecord {
        timestamp: Utc::now(),
        turn,
        provider: "cli:test".to_string(),
        model: "test".to_string(),
        input_tokens: turn as u64,
        output_tokens: 2 * turn as u64,
        cost_usd: 0.0,
        total_cost_usd: 0.0,
        cap_usd: None,
        subscription: false,
        estimated: false,
        wall_time_seconds: None,
        wall_time_cap_seconds: None,
    }
}

fn trace_record(run_id: &str, turn: u32) -> TraceRecord {
    TraceRecord {
        timestamp: Utc::now(),
        run_id: run_id.to_string(),
        turn,
        event: "test".to_string(),
        latency_ms: None,
        detail: serde_json::json!({ "turn": turn }),
    }
}

fn flight_event(run_id: &str, seq: u64, usage: Option<FlightUsage>) -> FlightEvent {
    FlightEvent {
        version: 1,
        seq,
        run_id: run_id.to_string(),
        flight_session_id: "flight-test".to_string(),
        deadreckon_turn: 1,
        attempt: 1,
        provider: "cli:test".to_string(),
        schema: "test".to_string(),
        timestamp: Some(Utc::now()),
        source_path: None,
        source_line: Some(seq),
        source_event: format!("event-{seq}"),
        raw_hash: format!("hash-{seq}"),
        kind: FlightEventKind::Tool,
        role: None,
        summary: format!("tool event {seq}"),
        tool_name: Some("edit".to_string()),
        tool_category: Some("write".to_string()),
        files: Vec::new(),
        usage,
        checkpoint_id: None,
    }
}

fn provider_log_spec(
    root: &std::path::Path,
    state: &deadreckon_core::PipelineState,
) -> ProviderJsonlLogSpec {
    ProviderJsonlLogSpec {
        schema: "codex-cli".to_string(),
        roots: vec![root.to_path_buf()],
        since: state.started_at - ChronoDuration::minutes(1),
        cwd_match: IngestCwdMatch::SessionMeta,
        cwd_match_path: None,
        storage: IngestStorage::Jsonl,
        file_glob: None,
    }
}

fn write_codex_provider_log(path: &std::path::Path, cwd: &std::path::Path, message: &str) {
    append_jsonl_value(
        path,
        &serde_json::json!({
            "type": "session_meta",
            "payload": { "cwd": cwd.to_string_lossy() },
        }),
    );
    append_codex_agent_message(path, message);
}

fn append_codex_agent_message(path: &std::path::Path, message: &str) {
    append_jsonl_value(
        path,
        &serde_json::json!({
            "timestamp": "2026-05-26T00:00:00Z",
            "type": "event_msg",
            "payload": {
                "type": "agent_message",
                "message": message,
            },
        }),
    );
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("")
}

fn test_tempdir() -> tempfile::TempDir {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.test-tmp");
    std::fs::create_dir_all(&root).expect("test tmp");
    tempfile::TempDir::new_in(root).expect("temp")
}

#[test]
fn command_help_catalog_rows_are_unique() {
    let mut top_rows = std::collections::BTreeSet::new();
    let mut help_all_rows = std::collections::BTreeSet::new();

    for entry in COMMAND_HELP_CATALOG {
        if entry.top_group.is_some() {
            assert!(
                top_rows.insert(entry.display),
                "duplicate top-help row for {}",
                entry.display
            );
        }
        if entry.all_group.is_some() {
            assert!(
                help_all_rows.insert(entry.display),
                "duplicate help-all row for {}",
                entry.display
            );
        }
    }

    assert!(top_rows.contains("help-all"));
    assert!(top_rows.contains("<command> --help"));
    assert!(help_all_rows.contains("export"));
    assert!(!help_all_rows.contains("materialize"));
}

#[test]
fn command_help_catalog_points_at_real_clap_commands() {
    let handle = std::thread::Builder::new()
        .name("command-help-catalog-clap-tree".to_string())
        // Clap's generated command tree is large enough to overflow the
        // default libtest worker stack on macOS.
        .stack_size(8 * 1024 * 1024)
        .spawn(assert_command_help_catalog_points_at_real_clap_commands)
        .expect("spawn clap command tree test");
    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

fn assert_command_help_catalog_points_at_real_clap_commands() {
    let clap_names = Cli::command()
        .get_subcommands()
        .map(|command| command.get_name().to_string())
        .collect::<std::collections::BTreeSet<_>>();

    for CommandHelpEntry {
        display, clap_name, ..
    } in COMMAND_HELP_CATALOG
    {
        let Some(clap_name) = clap_name else {
            continue;
        };
        assert!(
            clap_names.contains(*clap_name),
            "catalog row {display} points at missing clap command {clap_name}"
        );
    }
}

#[test]
fn command_help_catalog_covers_expected_sections() {
    let mut top_groups = std::collections::BTreeSet::new();
    let mut all_groups = std::collections::BTreeSet::new();
    for entry in COMMAND_HELP_CATALOG {
        if let Some(group) = entry.top_group {
            top_groups.insert(format!("{group:?}"));
        }
        if let Some(group) = entry.all_group {
            all_groups.insert(format!("{group:?}"));
        }
    }

    for group in [
        TopHelpGroup::StartWatchKeep,
        TopHelpGroup::SetupHealth,
        TopHelpGroup::Control,
        TopHelpGroup::FindMore,
    ] {
        assert!(top_groups.contains(&format!("{group:?}")));
    }
    for (group, _) in HELP_ALL_GROUPS {
        assert!(all_groups.contains(&format!("{group:?}")));
    }
}

#[test]
fn command_help_catalog_classifies_advanced_and_compatibility_surfaces() {
    let entry = |name: &str| {
        COMMAND_HELP_CATALOG
            .iter()
            .find(|entry| entry.display == name)
            .unwrap_or_else(|| panic!("missing catalog row {name}"))
    };

    for name in [
        "run",
        "orchestrate",
        "chain",
        "extend",
        "apply",
        "export",
        "abandon",
        "doc",
        "show",
    ] {
        assert_eq!(command_discovery(entry(name)), CommandDiscovery::Advanced);
        assert_eq!(
            entry(name).top_group,
            None,
            "{name} should stay out of short help"
        );
    }
    assert_eq!(
        command_discovery(entry("acceptance")),
        CommandDiscovery::Compatibility
    );
    for name in [
        "start", "attach", "status", "list", "finish", "doctor", "kill", "resume", "cleanup",
    ] {
        assert_eq!(command_discovery(entry(name)), CommandDiscovery::Public);
        assert_eq!(
            entry(name).audience,
            CommandAudience::Primary,
            "{name} should be part of the primary production model"
        );
        assert!(
            entry(name).top_group.is_some(),
            "{name} should be in short help"
        );
    }
    for name in ["init", "def-done"] {
        assert_eq!(command_discovery(entry(name)), CommandDiscovery::Public);
        assert_eq!(entry(name).audience, CommandAudience::SetupSupport);
        assert!(
            entry(name).top_group.is_some(),
            "{name} should support setup"
        );
    }
    assert!(
        COMMAND_HELP_CATALOG
            .iter()
            .all(|entry| entry.display != "materialize"),
        "materialize must stay an inline compatibility alias, not a catalog row"
    );
}

#[test]
fn attach_banner_names_kind_and_prefix() {
    let id = "aaaabbbbccccdddd1111222233334444";
    assert_eq!(attach_banner("run", id), "attaching to run aaaabbbb");
    assert_eq!(attach_banner("chain", id), "attaching to chain aaaabbbb");
    assert_eq!(attach_banner("plan", id), "attaching to plan aaaabbbb");
}

#[test]
fn kill_banner_names_kind_prefix_and_plan_process_count() {
    assert_eq!(
        kill_banner("run", "aaaabbbb", false, None),
        "killed run aaaabbbb"
    );
    assert_eq!(
        kill_banner("chain", "aaaabbbb", true, None),
        "killed chain aaaabbbb forcefully"
    );
    assert_eq!(
        kill_banner("plan", "aaaabbbb", true, Some(3)),
        "killed plan aaaabbbb forcefully (3 processes signalled)"
    );
}

fn doc_preview_state() -> (tempfile::TempDir, deadreckon_core::PipelineState) {
    let temp = test_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = create_run(
        &paths,
        RunOptions {
            goal: "preview docs".to_string(),
            cwd: temp.path().to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some("cli:codex".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("state");
    (temp, state)
}

fn full_plan_fixture(task_count: usize) -> (tempfile::TempDir, DeadreckonPaths, Plan) {
    let temp = test_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let tasks = (0..task_count)
        .map(|index| {
            PlanTask::new(
                index as u32,
                format!("Task {index}"),
                format!("Do child {index}"),
                PlanRole::Child,
                Some(if index == 1 {
                    "smoke:reviewer".to_string()
                } else {
                    "smoke:child".to_string()
                }),
            )
        })
        .collect::<Vec<_>>();
    let mut plan = Plan::new(
        "build orchestrated app",
        PlanMode::FullPlan,
        tasks,
        PlanProviders {
            planner: Some("smoke:planner".to_string()),
            default_child: Some("smoke:child".to_string()),
            coder: None,
            reviewer: None,
            children: [(1, "smoke:reviewer".to_string())].into(),
        },
        Some("scope".to_string()),
        "0.1.0",
    )
    .expect("plan");
    plan.capability_preview = CapabilityPreview {
        network: NetworkCapability::Allowlist,
        deploy: true,
        global_install: false,
        filesystem: vec!["working directory".to_string()],
        notes: Vec::new(),
    };
    (temp, paths, plan)
}

fn review_plan_fixture() -> (tempfile::TempDir, DeadreckonPaths, Plan) {
    let temp = test_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let coder = PlanTask::new(
        0,
        "Implement requested change",
        "Build the change",
        PlanRole::Coder,
        Some("smoke:coder".to_string()),
    );
    let mut reviewer = PlanTask::new(
        1,
        "Review and fix implementation",
        "Review the coder result",
        PlanRole::Reviewer,
        Some("smoke:reviewer".to_string()),
    );
    reviewer.depends_on = vec![coder.task_id.clone()];
    let plan = Plan::new(
        "tiny hello rust",
        PlanMode::Review,
        vec![coder, reviewer],
        PlanProviders {
            planner: None,
            default_child: None,
            coder: Some("smoke:coder".to_string()),
            reviewer: Some("smoke:reviewer".to_string()),
            children: Default::default(),
        },
        Some("scope".to_string()),
        "0.1.0",
    )
    .expect("plan");
    (temp, paths, plan)
}

fn attach_child_run_with_docs(
    paths: &DeadreckonPaths,
    temp: &tempfile::TempDir,
    plan: &mut Plan,
    index: usize,
    narrative: &str,
) -> deadreckon_core::PipelineState {
    let state = create_run(
        paths,
        RunOptions {
            goal: format!("child {index}"),
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
    .expect("child run");
    let docs = state.working_dir.join(".deadreckon/docs");
    std::fs::create_dir_all(&docs).expect("docs dir");
    std::fs::write(docs.join(deadreckon_core::RUN_NARRATIVE), narrative).expect("narrative");
    std::fs::write(
        docs.join(deadreckon_core::RUN_AS_BUILT),
        format!("# Child {index} As Built\n\nBuilt file `child-{index}.txt`."),
    )
    .expect("as built");
    std::fs::write(
        docs.join(deadreckon_core::RUN_DECISIONS),
        format!("# Child {index} Decisions\n\nChose child {index} implementation."),
    )
    .expect("decisions");
    std::fs::write(
        state.working_dir.join(format!("child-{index}.txt")),
        format!("child {index} artifact"),
    )
    .expect("artifact");
    let task = plan.tasks.get_mut(index).expect("task");
    task.child_run_id = Some(state.run_id.clone());
    task.status = PlanTaskStatus::Completed;
    state
}

fn write_plan_specs_and_summaries(paths: &DeadreckonPaths, plan: &Plan) {
    for task in &plan.tasks {
        write_worker_spec(
            paths,
            &plan.plan_id,
            &task.task_id,
            &format!("# Worker {}\n\nImplement {}.", task.task_id, task.subject),
        )
        .expect("worker");
        write_child_summary(
            paths,
            &plan.plan_id,
            &task.task_id,
            &format!("Summary for {} with useful plan evidence.", task.task_id),
        )
        .expect("summary");
    }
}

#[test]
fn plan_docs_collect_child_run_docs_in_task_graph_order() {
    let (temp, paths, mut plan) = full_plan_fixture(3);
    plan.tasks[0].depends_on = vec!["task-1".to_string()];
    write_plan_specs_and_summaries(&paths, &plan);
    attach_child_run_with_docs(
        &paths,
        &temp,
        &mut plan,
        0,
        "# Child 0 Narrative\n\nImplemented task zero with enough detail to be useful.",
    );
    attach_child_run_with_docs(
        &paths,
        &temp,
        &mut plan,
        1,
        "# Child 1 Narrative\n\nImplemented dependency task one with enough detail.",
    );

    let input = collect_plan_doc_input(&paths, &plan).expect("input");

    assert_eq!(input.task_order, vec!["task-1", "task-0", "task-2"]);
    let task0 = input
        .children
        .iter()
        .find(|child| child.task_id == "task-0")
        .expect("task0");
    assert_eq!(task0.doc_status, "polished");
    assert!(
        task0
            .docs
            .iter()
            .any(|doc| doc.evidence_id == "doc:task-0:narrative"),
        "{task0:#?}"
    );
}

#[test]
fn plan_docs_fallback_writes_narrative_as_built_decisions_and_children() {
    let (temp, paths, mut plan) = full_plan_fixture(2);
    plan.status = PlanStatus::Merged;
    plan.merged_run_id = Some("11112222333344445555666677778888".to_string());
    write_plan_specs_and_summaries(&paths, &plan);
    attach_child_run_with_docs(
        &paths,
        &temp,
        &mut plan,
        0,
        "# Child 0 Narrative\n\nImplemented game levels with enough concrete detail.",
    );
    attach_child_run_with_docs(
        &paths,
        &temp,
        &mut plan,
        1,
        "# Child 1 Narrative\n\nImplemented scoreboard with enough concrete detail.",
    );
    let merge_working = paths.merge_working(&plan.plan_id);
    std::fs::create_dir_all(&merge_working).expect("merge working");
    std::fs::write(merge_working.join("index.html"), "<main>game</main>").expect("result");

    let manifest = write_plan_docs_deterministic(&paths, &plan, None, "none", None).expect("docs");

    assert_eq!(manifest.children.len(), 2);
    for name in [
        deadreckon_core::plan::PLAN_NARRATIVE,
        PLAN_AS_BUILT,
        PLAN_DECISIONS,
        PLAN_CHILDREN,
    ] {
        let path = plan_doc_path(&paths, &plan.plan_id, name);
        assert!(path.exists(), "missing {}", path.display());
        let raw = std::fs::read_to_string(path).expect("doc");
        assert!(raw.contains("task-0"), "{raw}");
    }
    let as_built = std::fs::read_to_string(plan_doc_path(&paths, &plan.plan_id, PLAN_AS_BUILT))
        .expect("as built");
    assert!(as_built.contains("index.html"), "{as_built}");
}

#[tokio::test]
async fn plan_docs_provider_over_budget_falls_back_to_deterministic_docs() {
    let (temp, paths, mut plan) = full_plan_fixture(2);
    plan.status = PlanStatus::Merged;
    plan.merged_run_id = Some("11112222333344445555666677778888".to_string());
    write_plan_specs_and_summaries(&paths, &plan);
    attach_child_run_with_docs(
        &paths,
        &temp,
        &mut plan,
        0,
        "# Child 0 Narrative\n\nImplemented levels with concrete details.",
    );

    let manifest = refresh_plan_docs(
        &paths,
        &plan,
        PlanDocRefreshOptions {
            provider: Some("openai".to_string()),
            provider_source: "flag".to_string(),
            budget_cap_usd: Some(0.0),
            force: true,
        },
    )
    .await
    .expect("docs");

    assert_eq!(manifest.status, "failed_provider_fallback");
    assert_eq!(manifest.provider.calls, 0);
    assert!(
        manifest
            .warnings
            .iter()
            .any(|warning| warning.contains("above cap")),
        "{manifest:#?}"
    );
    assert!(plan_doc_path(&paths, &plan.plan_id, PLAN_DOC_PROVIDER_ERROR).exists());
    assert!(plan_doc_path(&paths, &plan.plan_id, deadreckon_core::plan::PLAN_NARRATIVE).exists());
}

#[test]
fn plan_result_apply_docs_do_not_replace_plan_rollup_with_empty_run_docs() {
    let (temp, paths, mut plan) = full_plan_fixture(2);
    plan.status = PlanStatus::Merged;
    plan.merged_run_id = Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string());
    write_plan_specs_and_summaries(&paths, &plan);
    attach_child_run_with_docs(
        &paths,
        &temp,
        &mut plan,
        0,
        "# Child 0 Narrative\n\nImplemented the whole app.",
    );
    write_plan_docs_deterministic(&paths, &plan, None, "none", None).expect("docs");
    let dest = temp.path().join("apply-worktree");
    std::fs::create_dir_all(dest.join(".deadreckon/docs")).expect("dest");
    std::fs::write(
        dest.join(".deadreckon/docs")
            .join(deadreckon_core::RUN_NARRATIVE),
        "# Empty\n\nNo completed turns have been recorded yet.",
    )
    .expect("empty");

    materialize_plan_docs_to_working(
        &paths,
        &plan,
        &dest,
        Some(&PlanWrapperDocContext {
            wrapper_run_id: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            merged_run_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        }),
    )
    .expect("materialize");

    let wrapper = std::fs::read_to_string(
        dest.join(".deadreckon/docs")
            .join(deadreckon_core::RUN_NARRATIVE),
    )
    .expect("wrapper");
    assert!(wrapper.contains("Plan Result Wrapper"), "{wrapper}");
    assert!(wrapper.contains("PLAN-NARRATIVE.md"), "{wrapper}");
    assert!(dest.join("docs").join("PLAN-NARRATIVE.md").exists());
    assert!(
        dest.join(".deadreckon/docs")
            .join("PLAN-CHILDREN.md")
            .exists()
    );
}

#[test]
fn plan_docs_provider_response_rejects_unknown_citations() {
    let (temp, paths, mut plan) = full_plan_fixture(2);
    write_plan_specs_and_summaries(&paths, &plan);
    attach_child_run_with_docs(
        &paths,
        &temp,
        &mut plan,
        0,
        "# Child 0 Narrative\n\nImplemented the app with enough detail.",
    );
    let input = collect_plan_doc_input(&paths, &plan).expect("input");
    let docs = PlanProviderDocs {
        schema_version: 1,
        title: "Plan".to_string(),
        narrative: PlanProviderNarrative {
            summary: "Summary".to_string(),
            task_graph: vec![PlanProviderItem {
                title: None,
                text: "Invented claim".to_string(),
                citations: vec!["unknown:evidence".to_string()],
            }],
            phases: Vec::new(),
            repairs: Vec::new(),
            acceptance: Vec::new(),
            open_threads: Vec::new(),
        },
        as_built: PlanProviderAsBuilt {
            system_overview: "System".to_string(),
            components: Vec::new(),
            changed_files: Vec::new(),
            runtime_notes: Vec::new(),
        },
        decisions: PlanProviderDecisions {
            decisions: Vec::new(),
            tradeoffs: Vec::new(),
            deferrals: Vec::new(),
        },
        children: vec![PlanProviderChild {
            task_id: "task-0".to_string(),
            summary: "Covered".to_string(),
            citations: vec!["task:task-0".to_string()],
        }],
    };

    let err = validate_plan_provider_docs(&input, &docs).expect_err("validation");

    assert!(err.to_string().contains("unknown plan doc citation"));
}

#[test]
fn plan_docs_provider_consolidates_child_docs_with_citations() {
    let (temp, paths, mut plan) = full_plan_fixture(2);
    write_plan_specs_and_summaries(&paths, &plan);
    attach_child_run_with_docs(
        &paths,
        &temp,
        &mut plan,
        0,
        "# Child 0 Narrative\n\nImplemented the app with enough detail.",
    );
    let input = collect_plan_doc_input(&paths, &plan).expect("input");
    let docs = PlanProviderDocs {
        schema_version: 1,
        title: "Consolidated plan".to_string(),
        narrative: PlanProviderNarrative {
            summary: "The plan completed the app.".to_string(),
            task_graph: vec![PlanProviderItem {
                title: Some("Task 0".to_string()),
                text: "Task 0 supplied the implementation.".to_string(),
                citations: vec!["task:task-0".to_string()],
            }],
            phases: Vec::new(),
            repairs: Vec::new(),
            acceptance: Vec::new(),
            open_threads: Vec::new(),
        },
        as_built: PlanProviderAsBuilt {
            system_overview: "One child produced the result.".to_string(),
            components: Vec::new(),
            changed_files: Vec::new(),
            runtime_notes: Vec::new(),
        },
        decisions: PlanProviderDecisions {
            decisions: vec![PlanProviderItem {
                title: None,
                text: "Kept the deterministic child artifact.".to_string(),
                citations: vec!["doc:task-0:decisions".to_string()],
            }],
            tradeoffs: Vec::new(),
            deferrals: Vec::new(),
        },
        children: vec![PlanProviderChild {
            task_id: "task-0".to_string(),
            summary: "Task 0 is covered.".to_string(),
            citations: vec!["task:task-0".to_string()],
        }],
    };

    validate_plan_provider_docs(&input, &docs).expect("valid");
    write_plan_docs_from_provider(&paths, &plan, &input, &docs).expect("write");

    let narrative = std::fs::read_to_string(plan_doc_path(
        &paths,
        &plan.plan_id,
        deadreckon_core::plan::PLAN_NARRATIVE,
    ))
    .expect("narrative");
    assert!(narrative.contains("Consolidated plan"), "{narrative}");
    assert!(narrative.contains("`task:task-0`"), "{narrative}");
}

#[test]
fn plan_docs_resolve_apply_wrapper_back_to_plan_and_merged_run() {
    let (temp, paths, mut plan) = full_plan_fixture(2);
    plan.status = PlanStatus::Merged;
    plan.merged_run_id = Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string());
    save_plan(&paths, &plan).expect("save plan");
    let wrapper = create_run(
        &paths,
        RunOptions {
            goal: "wrapper".to_string(),
            cwd: temp.path().to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some("deadreckon:orchestrate-apply".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("wrapper");
    append_trace(
        &wrapper,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: wrapper.run_id.clone(),
            turn: 0,
            event: "plan_result_apply_prepared".to_string(),
            latency_ms: None,
            detail: serde_json::json!({
                "plan_id": plan.plan_id,
                "merged_run_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            }),
        },
    )
    .expect("trace");

    let target = resolve_plan_doc_target(&paths, &wrapper.run_id, Some(&wrapper)).expect("target");

    let target = target.expect("plan target");
    assert_eq!(target.plan.plan_id, plan.plan_id);
    assert_eq!(
        target.wrapper.expect("wrapper").merged_run_id,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
}

fn render_plan_attach_text(
    paths: &DeadreckonPaths,
    plan: &Plan,
    messages: &[PlanMessage],
    plan_events: &[PlanEvent],
    selected: usize,
) -> String {
    render_plan_attach_text_with_feed(paths, plan, messages, plan_events, &[], selected)
}

fn render_plan_attach_text_with_view(
    paths: &DeadreckonPaths,
    plan: &Plan,
    messages: &[PlanMessage],
    plan_events: &[PlanEvent],
    selected: usize,
    view: AttachViewMode,
    visual: NarrativeVisualMode,
) -> String {
    render_plan_attach_text_with_state(
        paths,
        plan,
        messages,
        plan_events,
        &[],
        PlanAttachRenderState {
            messages,
            plan_events,
            feed_events: &[],
            selected,
            show_hints: true,
            view,
            visual,
            campaign_parent: None,
            narrative_notice: None,
            narrative_projection: None,
        },
    )
}

fn render_plan_attach_text_with_feed(
    paths: &DeadreckonPaths,
    plan: &Plan,
    messages: &[PlanMessage],
    plan_events: &[PlanEvent],
    feed_events: &[PlanFeedEvent],
    selected: usize,
) -> String {
    render_plan_attach_text_with_state(
        paths,
        plan,
        messages,
        plan_events,
        feed_events,
        PlanAttachRenderState {
            messages,
            plan_events,
            feed_events,
            selected,
            show_hints: true,
            view: AttachViewMode::Activity,
            visual: NarrativeVisualMode::Architecture,
            campaign_parent: None,
            narrative_notice: None,
            narrative_projection: None,
        },
    )
}

fn render_plan_attach_text_with_state(
    paths: &DeadreckonPaths,
    plan: &Plan,
    _messages: &[PlanMessage],
    _plan_events: &[PlanEvent],
    _feed_events: &[PlanFeedEvent],
    state: PlanAttachRenderState<'_>,
) -> String {
    let backend = TestBackend::new(140, 34);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render_plan_attach(frame, paths, plan, &state))
        .expect("draw");
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

fn render_attach_text(
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    live: &AttachLive,
) -> String {
    render_attach_text_with_tui_state(state, spend, live, AttachTuiState::default())
}

fn render_attach_text_with_tui_state(
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    live: &AttachLive,
    tui_state: AttachTuiState,
) -> String {
    render_attach_text_with_size(state, spend, live, tui_state, 140, 34)
}

fn render_attach_text_with_size(
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    live: &AttachLive,
    tui_state: AttachTuiState,
    width: u16,
    height: u16,
) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render_attach(frame, state, spend, &[], &[], live, &tui_state))
        .expect("draw");
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

fn render_chain_attach_text(
    chain: &Chain,
    events: &[ChainEvent],
    tui_state: &ChainAttachTuiState,
) -> String {
    let backend = TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render_chain_attach(frame, chain, events, tui_state))
        .expect("draw");
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

struct ScriptedStartPrompter {
    choices: VecDeque<String>,
    inputs: VecDeque<String>,
    confirms: VecDeque<bool>,
    prompt_titles: Vec<String>,
}

impl ScriptedStartPrompter {
    fn new(choices: &[&str]) -> Self {
        Self {
            choices: choices.iter().map(|choice| choice.to_string()).collect(),
            inputs: VecDeque::new(),
            confirms: VecDeque::new(),
            prompt_titles: Vec::new(),
        }
    }
}

impl StartPrompter for ScriptedStartPrompter {
    fn select_one(
        &mut self,
        prompt: crate::prompt::SelectPrompt,
    ) -> Result<crate::prompt::SelectChoice> {
        self.prompt_titles.push(prompt.title.clone());
        let id = self.choices.pop_front().expect("scripted choice");
        Ok(prompt
            .choices
            .into_iter()
            .find(|choice| choice.id == id)
            .unwrap_or_else(|| panic!("choice {id} not found")))
    }

    fn confirm(&mut self, question: &str, _default_yes: bool) -> Result<bool> {
        self.prompt_titles.push(question.to_string());
        Ok(self.confirms.pop_front().unwrap_or(true))
    }

    fn input(&mut self, message: &str, _default: Option<&str>) -> Result<String> {
        self.prompt_titles.push(message.to_string());
        Ok(self.inputs.pop_front().unwrap_or_default())
    }
}

fn start_args_for_test(goal: &str) -> StartCommandArgs {
    StartCommandArgs {
        goal: goal.to_string(),
        mode: CliStartMode::Auto,
        provider: None,
        children: None,
        planner_provider: None,
        child_provider: Vec::new(),
        coder_provider: None,
        reviewer_provider: None,
        preview: true,
        yes: false,
        no_seams: false,
        fresh: false,
        worktree: false,
        from: None,
        allow_dirty: false,
        plain: false,
        quiet: false,
        json: false,
    }
}

#[test]
fn goal_file_reads_document_goal() {
    let temp = test_tempdir();
    let goal_path = temp.path().join("goal.md");
    std::fs::write(
        &goal_path,
        "\u{feff}\nBuild the dashboard\n\nInclude persistence.\n",
    )
    .expect("write goal");

    let goal = super::resolve_required_goal_input(
        "run",
        None,
        Some(goal_path),
        "deadreckon run --goal-file docs/goal.md",
    )
    .expect("goal");

    assert_eq!(goal, "Build the dashboard\n\nInclude persistence.");
}

#[test]
fn at_goal_reads_document_goal() {
    let temp = test_tempdir();
    let goal_path = temp.path().join("goal.md");
    std::fs::write(&goal_path, "Build from shorthand\n").expect("write goal");

    let goal = super::resolve_required_goal_input(
        "start",
        Some(format!("@{}", goal_path.display())),
        None,
        "deadreckon start --goal-file docs/goal.md",
    )
    .expect("goal");

    assert_eq!(goal, "Build from shorthand");
}

#[test]
fn goal_input_rejects_positional_and_file() {
    let temp = test_tempdir();
    let goal_path = temp.path().join("goal.md");
    std::fs::write(&goal_path, "Build from file\n").expect("write goal");

    let err = super::resolve_required_goal_input(
        "campaign",
        Some("Build inline".to_string()),
        Some(goal_path),
        "deadreckon campaign --goal-file docs/goal.md",
    )
    .expect_err("conflict should fail");

    assert!(
        err.to_string()
            .contains("either a positional goal or --goal-file")
    );
}

#[test]
fn goal_file_resolution_uses_project_root_when_cwd_misses() {
    let temp = test_tempdir();
    let root = temp.path();
    let cwd = root.join("src").join("feature");
    let docs = root.join("docs");
    std::fs::create_dir_all(&cwd).expect("cwd");
    std::fs::create_dir_all(&docs).expect("docs");
    let goal_path = docs.join("goal.md");
    std::fs::write(&goal_path, "Build from project root\n").expect("write goal");

    let resolved =
        super::resolve_goal_file_path_from(std::path::Path::new("docs/goal.md"), &cwd, Some(root));

    assert_eq!(resolved, goal_path);
}

#[test]
fn goal_file_resolution_prefers_cwd_before_project_root() {
    let temp = test_tempdir();
    let root = temp.path();
    let cwd = root.join("src");
    std::fs::create_dir_all(root.join("docs")).expect("root docs");
    std::fs::create_dir_all(cwd.join("docs")).expect("cwd docs");
    let root_goal = root.join("docs").join("goal.md");
    let cwd_goal = cwd.join("docs").join("goal.md");
    std::fs::write(&root_goal, "Build from root\n").expect("write root goal");
    std::fs::write(&cwd_goal, "Build from cwd\n").expect("write cwd goal");

    let resolved =
        super::resolve_goal_file_path_from(std::path::Path::new("docs/goal.md"), &cwd, Some(root));

    assert_eq!(resolved, cwd_goal);
}

#[test]
fn goal_file_resolution_expands_home_prefix() {
    let temp = test_tempdir();
    let home = temp.path().join("home");

    let resolved =
        super::expand_goal_file_home(std::path::Path::new("~/goals/app.md"), Some(&home));

    assert_eq!(resolved, home.join("goals").join("app.md"));
}

#[test]
fn orchestration_mode_recommendation_prefers_full_plan_for_broad_products() {
    assert_eq!(
        recommend_orchestration_mode("make a fully multiplayer live flight simulator"),
        CliPlanMode::FullPlan
    );
    assert_eq!(
        recommend_orchestration_mode("fix the provider table spacing"),
        CliPlanMode::Review
    );
}

#[test]
fn start_auto_defaults_to_run_for_simple_goal() {
    let decision = start_launch_decision(StartLaunchInput {
        goal: "fix the provider table spacing",
        requested_mode: CliStartMode::Auto,
        stdin_is_tty: true,
    });

    assert_eq!(decision.selected_mode, StartSelectedMode::Run);
    assert_eq!(decision.selection_source, StartSelectionSource::Default);
    assert!(decision.reason.contains("single supervised run"));
}

#[test]
fn start_auto_recommends_review_for_review_goal() {
    let decision = start_launch_decision(StartLaunchInput {
        goal: "review and harden the provider setup flow",
        requested_mode: CliStartMode::Auto,
        stdin_is_tty: true,
    });

    assert_eq!(decision.selected_mode, StartSelectedMode::Review);
    assert_eq!(decision.selection_source, StartSelectionSource::Heuristic);
    assert!(decision.reason.contains("review"));
}

#[test]
fn start_auto_recommends_full_plan_for_parallel_goal() {
    let decision = start_launch_decision(StartLaunchInput {
        goal: "parallelize the API, frontend, docs, and release workstreams",
        requested_mode: CliStartMode::Auto,
        stdin_is_tty: true,
    });

    assert_eq!(decision.selected_mode, StartSelectedMode::FullPlan);
    assert_eq!(decision.selection_source, StartSelectionSource::Heuristic);
    assert!(decision.reason.contains("parallel"));
}

#[test]
fn goal_shape_classifier_validates_and_clamps_provider_output() {
    let recommendation = parse_provider_goal_shape(
        "rebuild billing, notifications, and admin",
        "cli:planner",
        r#"{"shape":"campaign","n":9,"rationale":"three independent services"}"#,
    )
    .expect("recommendation");

    assert_eq!(recommendation.shape, GoalShape::Campaign);
    assert_eq!(recommendation.n, Some(6));
    assert_eq!(recommendation.source, GoalShapeSource::Provider);
    assert_eq!(recommendation.provider.as_deref(), Some("cli:planner"));
    assert!(
        parse_provider_goal_shape(
            "bad",
            "cli:planner",
            r#"{"shape":"surprise","n":3,"rationale":"bad"}"#,
        )
        .is_none()
    );
}

#[test]
fn goal_shape_falls_back_to_deterministic_when_provider_unavailable() {
    let recommendation =
        fallback_goal_shape_recommendation("rebuild billing, notifications, and admin");

    assert_eq!(recommendation.shape, GoalShape::Campaign);
    assert_eq!(recommendation.n, Some(3));
    assert_eq!(recommendation.source, GoalShapeSource::Fallback);
    assert!(recommendation.rationale.contains("independent clauses"));
}

#[test]
fn single_change_goal_classifies_as_single() {
    let recommendation = fallback_goal_shape_recommendation("fix the login button spacing");

    assert_eq!(recommendation.shape, GoalShape::Single);
    assert_eq!(recommendation.n, None);
    assert!(
        recommendation
            .rationale
            .contains(&format!("one {NOUN_VERIFIED_RUN}"))
    );
}

#[test]
fn classified_campaign_shape_is_suggested_not_auto_launched() {
    let mut decision = start_launch_decision(StartLaunchInput {
        goal: "rebuild billing, notifications, and admin",
        requested_mode: CliStartMode::Auto,
        stdin_is_tty: true,
    });
    let recommendation = GoalShapeRecommendation {
        schema_version: 1,
        goal: decision.goal.clone(),
        shape: GoalShape::Campaign,
        n: Some(3),
        rationale: "three independent surfaces".to_string(),
        source: GoalShapeSource::Provider,
        provider: Some("cli:planner".to_string()),
    };

    apply_goal_shape_recommendation(&mut decision, recommendation);

    assert_eq!(decision.selected_mode, StartSelectedMode::Campaign);
    assert_eq!(decision.selection_source, StartSelectionSource::GoalShape);
    assert_eq!(decision.child_count, Some(3));
    assert!(!decision.confirmed_by_start_picker);
    assert!(!decision.requires_confirmation);
    let rows = launch_preview_rows(&start_launch_preview_facts(&decision));
    assert!(
        rows.iter()
            .any(|(key, value)| key == "suggestion" && value.contains("campaign n=3 via provider")),
        "{rows:?}"
    );
}

#[test]
fn campaign_preflight_can_drop_a_subgoal_before_launch() {
    let sub_goals = deadreckon_core::campaign::build_sub_goals(
        vec![
            "rebuild billing".to_string(),
            "rebuild notifications".to_string(),
            "rebuild admin".to_string(),
        ],
        3,
    )
    .expect("sub goals");
    let mut campaign = deadreckon_core::campaign::Campaign::new(
        "rebuild product surfaces",
        sub_goals,
        PlanProviders::default(),
        0,
        None,
        None,
        "0.1.0",
    )
    .expect("campaign");

    campaign_drop_subgoal_before_launch(&mut campaign, "sub-1").expect("drop");

    assert_eq!(campaign.n, 2);
    assert_eq!(campaign.sub_goals.len(), 2);
    assert_eq!(campaign.sub_goals[0].sub_id, "sub-0");
    assert_eq!(campaign.sub_goals[1].sub_id, "sub-1");
    assert_eq!(campaign.sub_goals[1].goal, "rebuild admin");
}

#[test]
fn campaign_preflight_can_edit_and_change_count_before_launch() {
    let sub_goals = deadreckon_core::campaign::build_sub_goals(
        vec![
            "rebuild billing".to_string(),
            "rebuild notifications".to_string(),
            "rebuild admin".to_string(),
        ],
        3,
    )
    .expect("sub goals");
    let mut campaign = deadreckon_core::campaign::Campaign::new(
        "rebuild product surfaces",
        sub_goals,
        PlanProviders::default(),
        0,
        None,
        None,
        "0.1.0",
    )
    .expect("campaign");

    campaign_edit_subgoal_before_launch(&mut campaign, "sub-0", "rebuild billing and plans")
        .expect("edit");
    assert_eq!(campaign.n, 3);
    assert_eq!(campaign.sub_goals[0].goal, "rebuild billing and plans");
    assert_eq!(campaign.sub_goals[0].sub_id, "sub-0");

    let replacement = deadreckon_core::campaign::build_sub_goals(
        vec![
            "rebuild billing".to_string(),
            "rebuild notifications".to_string(),
            "rebuild admin".to_string(),
            "rebuild docs".to_string(),
        ],
        4,
    )
    .expect("replacement");
    campaign_replace_sub_goals_before_launch(&mut campaign, replacement).expect("replace");
    assert_eq!(campaign.n, 4);
    assert_eq!(campaign.sub_goals.len(), 4);
    assert_eq!(campaign.sub_goals[3].sub_id, "sub-3");
    assert_eq!(campaign.sub_goals[3].goal, "rebuild docs");
}

#[test]
fn non_tty_ambiguous_auto_chooses_run() {
    let decision = start_launch_decision(StartLaunchInput {
        goal: "improve the app",
        requested_mode: CliStartMode::Auto,
        stdin_is_tty: false,
    });

    assert_eq!(decision.selected_mode, StartSelectedMode::Run);
    assert_eq!(decision.selection_source, StartSelectionSource::Default);
    assert!(decision.reason.contains("non-interactive"));
}

#[test]
fn start_prompt_eligibility_skips_scripted_modes() {
    assert!(
        StartPromptEligibility {
            stdin_is_tty: true,
            json: false,
            plain: false,
            quiet: false,
            yes: false,
        }
        .allows_prompts()
    );
    for eligibility in [
        StartPromptEligibility {
            stdin_is_tty: false,
            json: false,
            plain: false,
            quiet: false,
            yes: false,
        },
        StartPromptEligibility {
            stdin_is_tty: true,
            json: true,
            plain: false,
            quiet: false,
            yes: false,
        },
        StartPromptEligibility {
            stdin_is_tty: true,
            json: false,
            plain: true,
            quiet: false,
            yes: false,
        },
        StartPromptEligibility {
            stdin_is_tty: true,
            json: false,
            plain: false,
            quiet: true,
            yes: false,
        },
        StartPromptEligibility {
            stdin_is_tty: true,
            json: false,
            plain: false,
            quiet: false,
            yes: true,
        },
    ] {
        assert!(!eligibility.allows_prompts(), "{eligibility:?}");
    }
}

#[test]
fn start_fake_prompter_can_choose_review_mode() {
    let mut decision = start_launch_decision(StartLaunchInput {
        goal: "improve the app",
        requested_mode: CliStartMode::Auto,
        stdin_is_tty: true,
    });
    let args = start_args_for_test("improve the app");
    let mut prompter = ScriptedStartPrompter::new(&["review"]);

    maybe_prompt_start_mode(&mut decision, &args, None, &mut prompter).expect("mode prompt");

    assert_eq!(decision.selected_mode, StartSelectedMode::Review);
    assert_eq!(
        decision.selection_source,
        StartSelectionSource::InteractiveChoice
    );
    assert_eq!(prompter.prompt_titles, vec!["Choose launch path"]);
}

#[test]
fn start_fake_prompter_can_choose_extend_from_completed_history() {
    let parent = RunListEntry {
        run_id: "aaaabbbbccccdddd1111222233334444".to_string(),
        scope: "scope".to_string(),
        goal: "build the original app".to_string(),
        status: RunStatus::Completed,
        updated_at: Utc::now(),
        state_path: std::path::PathBuf::from("state.json"),
    };
    let mut decision = start_launch_decision(StartLaunchInput {
        goal: "add settings",
        requested_mode: CliStartMode::Auto,
        stdin_is_tty: true,
    });
    let args = start_args_for_test("add settings");
    let mut prompter = ScriptedStartPrompter::new(&["extend:aaaabbbbccccdddd1111222233334444"]);

    maybe_prompt_start_mode(&mut decision, &args, Some(&parent), &mut prompter)
        .expect("mode prompt");

    assert_eq!(decision.selected_mode, StartSelectedMode::Extend);
    assert_eq!(
        decision.selection_source,
        StartSelectionSource::InteractiveChoice
    );
    assert_eq!(decision.source_mode, StartSourceMode::ParentArtifact);
    assert_eq!(
        decision.base_run_id.as_deref(),
        Some("aaaabbbbccccdddd1111222233334444")
    );
    let rows = launch_preview_rows(&start_launch_preview_facts(&decision));
    assert!(
        rows.iter()
            .any(|(key, value)| key == "base" && value == "run aaaabbbb"),
        "{rows:?}"
    );
}

#[test]
fn start_history_actions_name_extend_and_new_orchestration_passes() {
    let parent = RunListEntry {
        run_id: "bbbbaaaaccccdddd1111222233334444".to_string(),
        scope: "scope".to_string(),
        goal: "build the original app".to_string(),
        status: RunStatus::Completed,
        updated_at: Utc::now(),
        state_path: std::path::PathBuf::from("state.json"),
    };
    let mut decision = start_launch_decision(StartLaunchInput {
        goal: "add charts",
        requested_mode: CliStartMode::Auto,
        stdin_is_tty: false,
    });

    add_start_history_actions(&mut decision, Some(&parent));

    assert_eq!(
        decision.history_action_label.as_deref(),
        Some("follow-up available from bbbbaaaa")
    );
    assert_eq!(
        decision.history_next_actions,
        vec![
            "deadreckon extend bbbbaaaa \"add charts\"".to_string(),
            "deadreckon start \"add charts\" --mode review --yes".to_string(),
            "deadreckon start \"add charts\" --mode full-plan --yes".to_string(),
        ]
    );
    let rows = launch_preview_rows(&start_launch_preview_facts(&decision));
    assert!(
        rows.iter()
            .any(|(key, value)| key == "history" && value == "follow-up available from bbbbaaaa"),
        "{rows:?}"
    );
}

#[test]
fn launch_preview_values_wrap_without_losing_words() {
    let lines = wrap_kv_value("deadreckon start \"add charts\" --mode full-plan --yes", 18);

    assert_eq!(
        lines,
        vec![
            "deadreckon start".to_string(),
            "\"add charts\"".to_string(),
            "--mode full-plan".to_string(),
            "--yes".to_string(),
        ]
    );
}

#[test]
fn start_full_plan_picker_collects_child_count_and_role_providers() {
    let temp = test_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path());
    let defaults = ConfigDefaults::default();
    let mut decision = start_launch_decision(StartLaunchInput {
        goal: "build a realtime 3d game",
        requested_mode: CliStartMode::FullPlan,
        stdin_is_tty: true,
    });
    decision.provider_source = StartProviderSource::Configured;
    decision.provider_route = Some("smoke".to_string());
    decision.provider_label = "smoke (configured)".to_string();
    let args = start_args_for_test("build a realtime 3d game");
    let mut prompter = ScriptedStartPrompter::new(&["n:5", "route:smoke", "route:smoke", "typed"]);
    prompter.inputs.push_back("1=smoke".to_string());

    resolve_start_orchestration_options(
        &mut decision,
        &args,
        &paths,
        &defaults,
        Some(&mut prompter),
    )
    .expect("orchestration options");

    assert_eq!(decision.child_count, Some(5));
    assert_eq!(decision.planner_provider_route.as_deref(), Some("smoke"));
    assert_eq!(decision.child_provider_route.as_deref(), Some("smoke"));
    assert_eq!(decision.child_provider_overrides, vec!["1=smoke"]);
    assert_eq!(
        prompter.prompt_titles,
        vec![
            "Choose child count",
            "Choose planner provider",
            "Choose default child provider",
            "Choose child provider overrides",
            "child provider overrides: "
        ]
    );
    assert_eq!(
        start_provider_role_summary(&decision).as_deref(),
        Some("children=5, planner=smoke, child=smoke, overrides=1=smoke")
    );
}

#[test]
fn start_fake_prompter_done_default_uses_default_gate() {
    let mut decision = start_launch_decision(StartLaunchInput {
        goal: "build the app",
        requested_mode: CliStartMode::Auto,
        stdin_is_tty: true,
    });
    let mut prompter = ScriptedStartPrompter::new(&["default"]);

    prompt_start_done_criteria(&mut decision, &mut prompter).expect("done prompt");

    assert_eq!(
        decision.done_criteria_source,
        StartDoneCriteriaSource::DefaultGate
    );
    assert_eq!(decision.done_action, StartDoneAction::DefaultGate);
    assert!(decision.done_criteria_label.contains("default"));
}

#[test]
fn start_fake_prompter_manual_done_creates_without_overwrite() {
    let mut decision = start_launch_decision(StartLaunchInput {
        goal: "build the app",
        requested_mode: CliStartMode::Auto,
        stdin_is_tty: true,
    });
    let mut prompter = ScriptedStartPrompter::new(&["manual"]);
    prompter
        .inputs
        .push_back("tests pass and screenshots are clean".to_string());

    prompt_start_done_criteria(&mut decision, &mut prompter).expect("done prompt");

    assert_eq!(
        start_done_materialization_request(&decision),
        Some(("tests pass and screenshots are clean".to_string(), false))
    );
}

#[test]
fn start_existing_done_criteria_prompt_can_keep_current_criteria() {
    let mut decision = start_launch_decision(StartLaunchInput {
        goal: "build the app",
        requested_mode: CliStartMode::Auto,
        stdin_is_tty: true,
    });
    let selection = crate::setup::DoneCriteriaSelection::project(
        std::path::PathBuf::from(".deadreckon/acceptance.yaml"),
        None,
        Some(2),
    );
    let mut prompter = ScriptedStartPrompter::new(&["keep"]);

    prompt_start_existing_done_criteria(
        &mut decision,
        std::path::Path::new("."),
        &selection,
        &mut prompter,
    )
    .expect("existing done criteria prompt");

    assert_eq!(
        decision.done_criteria_source,
        StartDoneCriteriaSource::Project
    );
    assert_eq!(decision.done_action, StartDoneAction::Existing);
    assert!(
        decision.done_criteria_label.contains("project (2 checks)"),
        "{}",
        decision.done_criteria_label
    );
}

#[test]
fn start_existing_done_criteria_prompt_can_update_before_launch() {
    let mut decision = start_launch_decision(StartLaunchInput {
        goal: "build the app",
        requested_mode: CliStartMode::Auto,
        stdin_is_tty: true,
    });
    let selection = crate::setup::DoneCriteriaSelection::project(
        std::path::PathBuf::from(".deadreckon/acceptance.yaml"),
        None,
        Some(1),
    );
    let mut prompter = ScriptedStartPrompter::new(&["update"]);
    prompter
        .inputs
        .push_back("tests pass and screenshots are clean".to_string());

    prompt_start_existing_done_criteria(
        &mut decision,
        std::path::Path::new("."),
        &selection,
        &mut prompter,
    )
    .expect("existing done criteria update prompt");

    assert_eq!(
        decision.done_criteria_source,
        StartDoneCriteriaSource::Manual
    );
    assert_eq!(
        decision.done_action,
        StartDoneAction::ManualText {
            text: "tests pass and screenshots are clean".to_string(),
            overwrite_existing: true,
        }
    );
    assert_eq!(
        start_done_materialization_request(&decision),
        Some(("tests pass and screenshots are clean".to_string(), true))
    );
    assert_eq!(
        decision.done_criteria_label,
        format!("update {NOUN_DONE_CONTRACT} before launch")
    );
}

#[test]
fn start_existing_done_criteria_prompt_can_view_then_update() {
    let mut decision = start_launch_decision(StartLaunchInput {
        goal: "build the app",
        requested_mode: CliStartMode::Auto,
        stdin_is_tty: true,
    });
    let selection = crate::setup::DoneCriteriaSelection::project(
        std::path::PathBuf::from(".deadreckon/acceptance.yaml"),
        None,
        Some(1),
    );
    let mut prompter = ScriptedStartPrompter::new(&["view", "update"]);
    prompter
        .inputs
        .push_back("browser loads and smoke tests pass".to_string());

    prompt_start_existing_done_criteria(
        &mut decision,
        std::path::Path::new("."),
        &selection,
        &mut prompter,
    )
    .expect("existing done criteria view/update prompt");

    assert_eq!(
        decision.done_action,
        StartDoneAction::ManualText {
            text: "browser loads and smoke tests pass".to_string(),
            overwrite_existing: true,
        }
    );
    assert_eq!(
        prompter.prompt_titles,
        vec![
            format!("Review {NOUN_DONE_CONTRACT}"),
            format!("Review {NOUN_DONE_CONTRACT}"),
            "updated definition of done: ".to_string()
        ]
    );
}

#[test]
fn orchestration_child_count_scales_with_goal_complexity() {
    assert_eq!(
        recommend_child_count_for_goal("fix a typo", CliPlanMode::Review),
        2
    );
    assert_eq!(
        recommend_child_count_for_goal(
            "make a multiplayer realtime physics terrain game with server",
            CliPlanMode::FullPlan
        ),
        5
    );
}

#[test]
fn orchestration_preflight_snapshot_captures_provider_roles_and_parallelism() {
    let (_temp, _paths, mut plan) = full_plan_fixture(3);
    plan.tasks[2].depends_on = vec!["task-0".to_string()];

    let role_lines =
        orchestration_role_table_lines(&orchestration_provider_role_rows(&plan, true, None));
    let parallelism = orchestration_parallelism_lines(&plan);

    assert!(
        role_lines
            .iter()
            .any(|line| line.contains("planner") && line.contains("smoke:planner")),
        "{role_lines:#?}"
    );
    assert!(
        role_lines
            .iter()
            .any(|line| line.contains("child task-1") && line.contains("smoke:reviewer")),
        "{role_lines:#?}"
    );
    assert!(
        role_lines
            .iter()
            .any(|line| line.contains("repair") && line.contains("smoke:planner")),
        "{role_lines:#?}"
    );
    assert!(
        parallelism
            .iter()
            .any(|line| line.contains("starts now: task-0, task-1")),
        "{parallelism:#?}"
    );
    assert!(
        parallelism
            .iter()
            .any(|line| line.contains("waits: task-2 after task-0")),
        "{parallelism:#?}"
    );
}

#[test]
fn orchestrate_preflight_prints_provider_role_table() {
    let (_temp, _paths, plan) = review_plan_fixture();

    let rows = orchestration_provider_role_rows(&plan, true, Some("smoke:repair"));

    assert!(
        rows.iter().any(|row| {
            row.role == "coder" && row.route == "smoke:coder" && row.source == "plan"
        })
    );
    assert!(rows.iter().any(|row| {
        row.role == "reviewer" && row.route == "smoke:reviewer" && row.source == "plan"
    }));
    assert!(rows.iter().any(|row| {
        row.role == "repair" && row.route == "smoke:repair" && row.source == "flag"
    }));
}

#[test]
fn orchestrate_preflight_names_ready_parallel_children() {
    let (_temp, _paths, mut plan) = full_plan_fixture(3);
    plan.tasks[1].depends_on = vec!["task-0".to_string()];
    plan.tasks[2].status = PlanTaskStatus::Completed;

    let rows = orchestration_dependency_rows(&plan);
    let lines = orchestration_parallelism_lines(&plan);

    assert!(
        rows.iter().any(|row| {
            row.child == "task-0" && row.starts == "now" && row.unblocks == "task-1"
        })
    );
    assert!(rows.iter().any(|row| {
        row.child == "task-1" && row.starts == "after task-0" && row.waits_for == "task-0"
    }));
    assert!(
        lines.iter().any(|line| line.contains("starts now: task-0")),
        "{lines:#?}"
    );
}

#[test]
fn merge_repair_summary_snapshot_captures_current_status() {
    let (_temp, paths, mut plan) = full_plan_fixture(2);
    plan.status = PlanStatus::Failed;
    save_plan(&paths, &plan).expect("save plan");
    let proofs = paths.merge_proofs(&plan.plan_id);
    std::fs::create_dir_all(&proofs).expect("proofs");
    std::fs::write(
        proofs.join("conflicts.json"),
        serde_json::json!({
            "schema_version": 2,
            "plan_id": plan.plan_id,
            "strategy": "dag-aware",
            "conflicts": [{ "path": "src/lib.rs" }]
        })
        .to_string(),
    )
    .expect("conflicts");
    std::fs::write(
        proofs.join("repair-request.json"),
        serde_json::json!({ "provider": "smoke:repair" }).to_string(),
    )
    .expect("request");
    std::fs::write(
        proofs.join("repair-plan.json"),
        serde_json::json!({
            "decision": "spawn_repair_child",
            "rationale": "needs integration"
        })
        .to_string(),
    )
    .expect("plan");
    std::fs::write(
        proofs.join("repair-run.json"),
        serde_json::json!({
            "run_id": "11112222333344445555666677778888",
            "status": "failed"
        })
        .to_string(),
    )
    .expect("run");
    append_plan_event(
        &paths,
        &plan.plan_id,
        PlanEventKind::MergeRepairPlanned {
            conflict_count: 1,
            provider: Some("smoke:repair".to_string()),
        },
    )
    .expect("planned");
    append_plan_event(
        &paths,
        &plan.plan_id,
        PlanEventKind::MergeRepairStarted {
            mode: "child".to_string(),
        },
    )
    .expect("started");

    let summary = plan_merge_repair_summary_items(&paths, &plan);

    assert!(
        summary
            .iter()
            .any(|(key, value)| key == "mode" && value == "child"),
        "{summary:#?}"
    );
    assert!(
        summary
            .iter()
            .any(|(key, value)| key == "provider" && value == "smoke:repair"),
        "{summary:#?}"
    );
    assert!(
        summary
            .iter()
            .any(|(key, value)| key == "conflicts" && value.contains("src/lib.rs")),
        "{summary:#?}"
    );
    assert!(
        summary
            .iter()
            .any(|(key, value)| key == "repair run" && value.contains("11112222 failed")),
        "{summary:#?}"
    );
    assert!(
        summary.iter().any(|(key, value)| key == "next action"
            && value.contains("deadreckon show")
            && value.contains("--why-failed")),
        "{summary:#?}"
    );
}

#[test]
fn preflight_warns_on_research_only_full_plan_tasks_for_build_goal() {
    let (_temp, _paths, mut plan) = full_plan_fixture(2);
    plan.root_goal = "make a fully multiplayer live flight simulator".to_string();
    plan.tasks[0].subject = "research flight sim architecture".to_string();
    plan.tasks[0].goal = "Research and document architecture options".to_string();
    plan.tasks[1].subject = "produce phased implementation roadmap".to_string();
    plan.tasks[1].goal = "Produce a roadmap document".to_string();

    let warnings = implementation_plan_warnings(&plan);

    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert!(warnings[0].contains("task-0"), "{warnings:#?}");
    assert!(warnings[0].contains("task-1"), "{warnings:#?}");
}

#[test]
fn attach_plan_shows_n_panes() {
    let (_temp, paths, plan) = full_plan_fixture(4);

    let text = render_plan_attach_text(&paths, &plan, &[], &[], 0);

    assert!(text.contains("task-0 pending"), "{text}");
    assert!(text.contains("task-1 pending"), "{text}");
    assert!(text.contains("task-2 pending"), "{text}");
    assert!(text.contains("task-3 pending"), "{text}");
    assert!(text.contains("children 0/0/4"), "{text}");
}

#[test]
fn attach_plan_shows_provider_and_role_per_pane() {
    let (_temp, paths, plan) = review_plan_fixture();

    let text = render_plan_attach_text(&paths, &plan, &[], &[], 1);

    assert!(
        text.contains("coder smoke:coder  reviewer smoke:reviewer"),
        "{text}"
    );
    assert!(text.contains("coder  provider smoke:coder"), "{text}");
    assert!(text.contains("reviewer  provider smoke:reviewer"), "{text}");
}

#[test]
fn attach_plan_shows_task_dependency_and_message_summary() {
    let (_temp, paths, mut plan) = full_plan_fixture(2);
    plan.tasks[1].depends_on = vec!["task-0".to_string()];
    plan.tasks[1].status = PlanTaskStatus::Failed;
    plan.status = PlanStatus::Forked;
    let message = PlanMessage::new(
        "coordinator",
        "task-1",
        PlanMessageKind::Blocker,
        "task-1 waiting on task-0",
        serde_json::json!({ "dependency": "task-0" }),
    )
    .expect("message");

    let text = render_plan_attach_text(&paths, &plan, &[message], &[], 1);

    assert!(text.contains("deps task-0"), "{text}");
    assert!(
        text.contains("coordinator -> task-1 Blocker: task-1 waiting on task-0"),
        "{text}"
    );
}

#[test]
fn attach_plan_prefers_plan_events_for_activity() {
    let (_temp, paths, plan) = full_plan_fixture(2);
    let event = PlanEvent {
        timestamp: Utc::now(),
        plan_id: plan.plan_id.clone(),
        event: PlanEventKind::TaskBlocked {
            task_id: "task-1".to_string(),
            task_index: 1,
            reason: "task-1 blocked by task-0".to_string(),
        },
    };

    let text = render_plan_attach_text(&paths, &plan, &[], &[event], 1);

    assert!(text.contains("plan events"), "{text}");
    assert!(
        text.contains("task-1 blocked: task-1 blocked by task-0"),
        "{text}"
    );
}

#[test]
fn plan_attach_tails_plan_events_without_restart() {
    let (_temp, paths, plan) = full_plan_fixture(2);
    save_plan(&paths, &plan).expect("save plan");
    append_plan_event(&paths, &plan.plan_id, PlanEventKind::PlanStarted).expect("append started");

    let text = render_plan_attach_text(
        &paths,
        &plan,
        &[],
        &read_plan_events_lossy(&paths, &plan.plan_id),
        0,
    );
    assert!(text.contains("plan started"), "{text}");

    append_plan_event(
        &paths,
        &plan.plan_id,
        PlanEventKind::TaskBlocked {
            task_id: "task-1".to_string(),
            task_index: 1,
            reason: "later event".to_string(),
        },
    )
    .expect("append blocked");
    let text = render_plan_attach_text(
        &paths,
        &plan,
        &[],
        &read_plan_events_lossy(&paths, &plan.plan_id),
        0,
    );

    assert!(text.contains("task-1 blocked: later event"), "{text}");
}

#[test]
fn plan_attach_activity_prefers_plan_events_over_messages() {
    let (_temp, paths, plan) = full_plan_fixture(2);
    let message = PlanMessage::new(
        "coordinator",
        "task-1",
        PlanMessageKind::Blocker,
        "message-only blocker",
        serde_json::json!({}),
    )
    .expect("message");
    let event = PlanEvent {
        timestamp: Utc::now(),
        plan_id: plan.plan_id.clone(),
        event: PlanEventKind::TaskBlocked {
            task_id: "task-1".to_string(),
            task_index: 1,
            reason: "event blocker".to_string(),
        },
    };

    let text = render_plan_attach_text(&paths, &plan, &[message], &[event], 1);

    assert!(text.contains("plan events"), "{text}");
    assert!(text.contains("event blocker"), "{text}");
    assert!(!text.contains("message-only blocker"), "{text}");
}

#[test]
fn attach_plan_receives_live_plan_child_and_repair_events() {
    let (_temp, paths, plan) = full_plan_fixture(2);
    let child_run_id = "11112222333344445555666677778888".to_string();
    let repair_run_id = "99998888777766665555444433332222".to_string();
    let feed_events = vec![
        PlanFeedEvent::Plan {
            event: PlanEvent {
                timestamp: Utc::now(),
                plan_id: plan.plan_id.clone(),
                event: PlanEventKind::PlanStarted,
            },
        },
        PlanFeedEvent::ChildRun {
            task_id: "task-0".to_string(),
            run_id: child_run_id.clone(),
            event: RunEvent {
                timestamp: Utc::now(),
                run_id: child_run_id,
                event: RunEventKind::TurnStarted { turn: 2 },
            },
        },
        PlanFeedEvent::RepairRun {
            run_id: repair_run_id.clone(),
            event: RunEvent {
                timestamp: Utc::now(),
                run_id: repair_run_id,
                event: RunEventKind::RunCompleted {
                    status: "completed".to_string(),
                },
            },
        },
    ];

    let text = render_plan_attach_text_with_feed(&paths, &plan, &[], &[], &feed_events, 0);

    assert!(text.contains("plan feed"), "{text}");
    assert!(text.contains("task-0"), "{text}");
    assert!(text.contains("turn 2 started"), "{text}");
    assert!(text.contains("repair run"), "{text}");
    assert!(text.contains("run completed"), "{text}");
}

#[test]
fn narrative_refresh_triggers_on_meaningful_run_events() {
    let run_id = "11112222333344445555666677778888".to_string();
    let benign = RunEvent {
        timestamp: Utc::now(),
        run_id: run_id.clone(),
        event: RunEventKind::TokenUsageDelta {
            turn: 1,
            input_tokens: 1,
            output_tokens: 1,
        },
    };
    assert_eq!(run_narrative_refresh_trigger(&[benign]), None);

    let error = RunEvent {
        timestamp: Utc::now(),
        run_id,
        event: RunEventKind::Error {
            turn: Some(1),
            message: "provider crashed".to_string(),
        },
    };

    assert_eq!(
        run_narrative_refresh_trigger(&[error]),
        Some(NarrativeRefreshKind::Event("run error"))
    );
}

#[test]
fn narrative_refresh_triggers_on_plan_and_child_milestones() {
    let (_temp, _paths, plan) = full_plan_fixture(2);
    let plan_event = PlanFeedEvent::Plan {
        event: PlanEvent {
            timestamp: Utc::now(),
            plan_id: plan.plan_id.clone(),
            event: PlanEventKind::TaskCompleted {
                task_id: "task-0".to_string(),
                task_index: 0,
                status: "completed".to_string(),
                run_id: Some("run-child".to_string()),
            },
        },
    };

    assert_eq!(
        plan_narrative_refresh_trigger(&[plan_event]),
        Some(NarrativeRefreshKind::Event("plan child completed"))
    );

    let child_event = PlanFeedEvent::ChildRun {
        task_id: "task-0".to_string(),
        run_id: "run-child".to_string(),
        event: RunEvent {
            timestamp: Utc::now(),
            run_id: "run-child".to_string(),
            event: RunEventKind::RunCompleted {
                status: "completed".to_string(),
            },
        },
    };

    assert_eq!(
        plan_narrative_refresh_trigger(&[child_event]),
        Some(NarrativeRefreshKind::Event("run completed"))
    );
}

#[test]
fn narrative_quiet_refresh_triggers_after_idle_running_period() {
    let start = Utc::now();
    let mut tracker = NarrativeQuietRefreshTracker::new(start);

    assert_eq!(
        tracker.maybe_trigger(true, 30, start + chrono::Duration::seconds(29)),
        None
    );
    assert_eq!(
        tracker.maybe_trigger(true, 30, start + chrono::Duration::seconds(31)),
        Some(NarrativeRefreshKind::QuietThreshold)
    );
    assert_eq!(
        tracker.maybe_trigger(true, 30, start + chrono::Duration::seconds(40)),
        None
    );
    assert_eq!(
        tracker.maybe_trigger(false, 30, start + chrono::Duration::seconds(70)),
        None
    );
}

#[test]
fn narrative_quiet_refresh_resets_on_meaningful_event() {
    let start = Utc::now();
    let mut tracker = NarrativeQuietRefreshTracker::new(start);
    tracker.observe_event_trigger(
        Some(NarrativeRefreshKind::Event("tool started")),
        start + chrono::Duration::seconds(20),
    );

    assert_eq!(
        tracker.maybe_trigger(true, 30, start + chrono::Duration::seconds(45)),
        None
    );
    assert_eq!(
        tracker.maybe_trigger(true, 30, start + chrono::Duration::seconds(51)),
        Some(NarrativeRefreshKind::QuietThreshold)
    );
}

#[test]
fn narrative_refresh_triggers_on_acceptance_status_changes() {
    let mut tracker = NarrativeAcceptanceRefreshTracker::default();
    let configured = AcceptanceLive {
        status: AcceptanceUiStatus::Configured,
        total: 2,
        completed: 0,
        passed: 0,
        failed: 0,
        required_failed: 0,
        latest_detail: None,
        progress_lines: Vec::new(),
    };
    assert_eq!(tracker.observe(&configured), None);

    let running = AcceptanceLive {
        status: AcceptanceUiStatus::Running,
        total: 2,
        completed: 1,
        passed: 1,
        failed: 0,
        required_failed: 0,
        latest_detail: Some("cargo test passed".to_string()),
        progress_lines: vec!["running 1/2".to_string()],
    };
    assert_eq!(
        tracker.observe(&running),
        Some(NarrativeRefreshKind::Event("acceptance running"))
    );
    assert_eq!(tracker.observe(&running), None);

    let passed = AcceptanceLive {
        status: AcceptanceUiStatus::Passed,
        total: 2,
        completed: 2,
        passed: 2,
        failed: 0,
        required_failed: 0,
        latest_detail: Some("all checks passed".to_string()),
        progress_lines: vec!["passed 2/2".to_string()],
    };
    assert_eq!(
        tracker.observe(&passed),
        Some(NarrativeRefreshKind::Event("acceptance passed"))
    );

    let failed = AcceptanceLive {
        status: AcceptanceUiStatus::Failed,
        total: 2,
        completed: 2,
        passed: 1,
        failed: 1,
        required_failed: 1,
        latest_detail: Some("cargo clippy failed".to_string()),
        progress_lines: vec!["failed 1/2".to_string()],
    };
    assert_eq!(
        tracker.observe(&failed),
        Some(NarrativeRefreshKind::Event("acceptance failed"))
    );
}

#[test]
fn narrative_provider_defaults_to_claude_code_sonnet_unless_overridden() {
    let default = narrative_provider_selection(None);
    assert_eq!(default.route.as_deref(), Some("cli:claude-code"));
    assert_eq!(default.model.as_deref(), Some("sonnet"));

    let explicit = narrative_provider_selection(Some("cli:codex"));
    assert_eq!(explicit.route.as_deref(), Some("cli:codex"));
    assert_eq!(explicit.model, None);

    let disabled = narrative_provider_selection(Some("none"));
    assert_eq!(disabled.route, None);
    assert_eq!(disabled.model, None);
}

#[test]
fn run_attach_narrative_pane_renders_headline_current_work_and_citations() {
    let (_temp, state) = doc_preview_state();
    let live = AttachLive {
        file_count: 1,
        total_bytes: 42,
        files: vec![LiveFile {
            path: "crates/deadreckon/src/main.rs".to_string(),
            bytes: 42,
            modified_at: None,
        }],
        provider_activity: vec!["tool edited crates/deadreckon/src/main.rs".to_string()],
        working_dir_exists: true,
        ..AttachLive::default()
    };
    let tui_state = AttachTuiState {
        view: AttachViewMode::Narrative,
        visual: NarrativeVisualMode::Architecture,
        ..AttachTuiState::default()
    };

    let text = render_attach_text_with_tui_state(&state, &[], &live, tui_state);

    assert!(text.contains("narrative"), "{text}");
    assert!(text.contains("Run "), "{text}");
    assert!(text.contains("Current work"), "{text}");
    assert!(text.contains("[file:"), "{text}");
    assert!(text.contains("crates/deadreckon/src/main.rs"), "{text}");
}

#[test]
fn run_attach_visual_cycle_preserves_scroll_and_footer() {
    let (_temp, state) = doc_preview_state();
    let mut tui_state = AttachTuiState {
        view: AttachViewMode::Narrative,
        visual: NarrativeVisualMode::Architecture,
        narrative_scroll: 3,
        ..AttachTuiState::default()
    };

    tui_state.cycle_visual();
    let text = render_attach_text_with_tui_state(
        &state,
        &[],
        &AttachLive::default(),
        AttachTuiState {
            view: tui_state.view,
            visual: tui_state.visual,
            narrative_scroll: tui_state.narrative_scroll,
            ..AttachTuiState::default()
        },
    );

    assert_eq!(tui_state.visual, NarrativeVisualMode::Agents);
    assert_eq!(tui_state.narrative_scroll, 3);
    assert!(text.contains("Visual=agents"), "{text}");
    assert!(text.contains("[n] Activity"), "{text}");
}

#[test]
fn run_attach_n_toggles_back_to_provider_activity() {
    let mut state = AttachTuiState {
        view: AttachViewMode::Narrative,
        docs_open: true,
        narrative_notice: Some("fresh".to_string()),
        ..AttachTuiState::default()
    };

    state.toggle_view();

    assert_eq!(state.view, AttachViewMode::Activity);
    assert!(!state.docs_open);
    assert!(state.narrative_notice.is_none());
    assert_eq!(state.focused_panel, AttachPanel::Activity);
}

#[test]
fn run_attach_completed_docs_toggle_still_reads_run_narrative_md() {
    let (_temp, mut state) = doc_preview_state();
    state.status = RunStatus::Completed;
    let docs_path = doc_path_for_kind(&state.working_dir, DocKind::Narrative).expect("doc path");
    std::fs::create_dir_all(docs_path.parent().expect("doc parent")).expect("doc dir");
    std::fs::write(
        &docs_path,
        "# Completed Narrative\n\nThe completed docs view remains distinct.",
    )
    .expect("doc");
    let tui_state = AttachTuiState {
        docs_open: true,
        view: AttachViewMode::Narrative,
        ..AttachTuiState::default()
    };

    let text = render_attach_text_with_tui_state(&state, &[], &AttachLive::default(), tui_state);

    assert!(text.contains("run docs / narrative"), "{text}");
    assert!(text.contains("Completed Narrative"), "{text}");
    assert!(!text.contains("freshness:"), "{text}");
}

#[test]
fn run_attach_narrow_terminal_keeps_footer_visible() {
    let (_temp, state) = doc_preview_state();
    let tui_state = AttachTuiState {
        view: AttachViewMode::Narrative,
        visual: NarrativeVisualMode::Evidence,
        ..AttachTuiState::default()
    };

    let text = render_attach_text_with_size(&state, &[], &AttachLive::default(), tui_state, 82, 22);

    assert!(text.contains("[n] Activity"), "{text}");
    assert!(text.contains("[r] Refresh"), "{text}");
    assert!(text.contains("q detach"), "{text}");
}

#[test]
fn plan_attach_narrative_renders_agent_table_and_visual() {
    let (_temp, paths, mut plan) = full_plan_fixture(2);
    plan.tasks[1].depends_on = vec!["task-0".to_string()];
    plan.tasks[0].status = PlanTaskStatus::Running;
    let event = PlanEvent {
        timestamp: Utc::now(),
        plan_id: plan.plan_id.clone(),
        event: PlanEventKind::TaskStarted {
            task_id: "task-0".to_string(),
            task_index: 0,
        },
    };

    let text = render_plan_attach_text_with_view(
        &paths,
        &plan,
        &[],
        &[event],
        0,
        AttachViewMode::Narrative,
        NarrativeVisualMode::Agents,
    );

    assert!(text.contains("plan narrative"), "{text}");
    assert!(text.contains("visual agents"), "{text}");
    assert!(text.contains("task-0"), "{text}");
    assert!(text.contains("smoke:child"), "{text}");
    assert!(text.contains("deps=1"), "{text}");
    assert!(text.contains("n narrative/activity"), "{text}");
}

#[test]
fn plain_narrative_attach_prints_staleness_and_citations() {
    let (_temp, state) = doc_preview_state();

    let text =
        run_narrative_plain_text(&state, None, NarrativeVisualMode::Architecture).expect("plain");

    assert!(text.contains("freshness: deterministic fallback"), "{text}");
    assert!(
        text.contains("Provider-backed narration has not run"),
        "{text}"
    );
    assert!(text.contains("Evidence"), "{text}");
    assert!(text.contains("file:"), "{text}");
}

#[test]
fn json_narrative_attach_emits_state_snapshot_and_graph_objects() {
    let (_temp, state) = doc_preview_state();

    let text =
        run_narrative_json_text(&state, None, NarrativeVisualMode::Architecture).expect("json");
    let value: serde_json::Value = serde_json::from_str(&text).expect("json value");

    assert_eq!(value["state"]["target_id"], state.run_id);
    assert_eq!(value["snapshot"]["target_id"], state.run_id);
    assert!(value["graph"]["nodes"].as_array().is_some(), "{value:#}");
    assert!(value["graph"]["edges"].as_array().is_some(), "{value:#}");
}

#[test]
fn non_tty_narrative_attach_does_not_call_provider_without_explicit_refresh() {
    let (_temp, state) = doc_preview_state();

    let _text =
        run_narrative_plain_text(&state, None, NarrativeVisualMode::Architecture).expect("plain");
    let narrative_state: crate::narrative::NarrativeState = serde_json::from_str(
        &std::fs::read_to_string(state.run_root.join("narrative/state.json")).expect("state"),
    )
    .expect("narrative state");

    assert_eq!(narrative_state.provider.calls, 0);
    assert_eq!(narrative_state.provider.source, "deterministic");
    assert!(
        !state
            .run_root
            .join("narrative/provider-refresh.out")
            .exists()
    );
}

#[test]
fn chain_narrative_attach_has_clear_supported_behavior() {
    let plain = chain_narrative_refusal_text("chain-1234", false).expect("plain");
    assert!(
        plain.starts_with("blocked chain narrative chain-1234"),
        "{plain}"
    );
    assert!(plain.contains("Explanation\n"), "{plain}");
    assert!(plain.contains("Evidence\n"), "{plain}");
    assert!(
        plain.contains("Chain narrative attach is not supported yet."),
        "{plain}"
    );
    assert_eq!(plain.matches("\nRecommended\n").count(), 1, "{plain}");
    assert!(
        plain.contains("Recommended\ndeadreckon chain status chain-1234"),
        "{plain}"
    );
    assert!(
        plain.contains("deadreckon attach <run-id> --view narrative"),
        "{plain}"
    );
    assert!(!plain.contains("try:"), "{plain}");

    let json_text = chain_narrative_refusal_text("chain-1234", true).expect("json");
    let value: serde_json::Value = serde_json::from_str(&json_text).expect("json value");
    assert_eq!(value["status"], "unsupported");
    assert_eq!(value["kind"], "chain");
    assert_eq!(
        value["primary_action"],
        "deadreckon chain status chain-1234"
    );
    assert_eq!(value["verdict"]["kind"], "blocked");
    assert_eq!(
        value["verdict"]["recommended_command"],
        "deadreckon chain status chain-1234"
    );
    assert!(
        value["try"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry == "deadreckon attach <plan-id> --view narrative"),
        "{value:#}"
    );
}

#[test]
fn plan_attach_handles_partial_plan_event_line() {
    let (_temp, paths, plan) = full_plan_fixture(2);
    save_plan(&paths, &plan).expect("save plan");
    append_plan_event(&paths, &plan.plan_id, PlanEventKind::PlanStarted).expect("append started");
    std::fs::OpenOptions::new()
        .append(true)
        .open(paths.plan_events(&plan.plan_id))
        .expect("open events")
        .write_all(b"{\"kind\":\"partial\"\n")
        .expect("partial line");

    let text = render_plan_attach_text(
        &paths,
        &plan,
        &[],
        &read_plan_events_lossy(&paths, &plan.plan_id),
        0,
    );

    assert!(text.contains("plan started"), "{text}");
}

#[test]
fn attach_plan_shows_capability_preview() {
    let (_temp, paths, plan) = full_plan_fixture(2);

    let text = render_plan_attach_text(&paths, &plan, &[], &[], 0);

    assert!(
        text.contains("capabilities network=Allowlist deploy=true install=false"),
        "{text}"
    );
    assert!(
        text.contains("planner smoke:planner  default child smoke:child"),
        "{text}"
    );
}

#[test]
fn attach_plan_enter_drills_then_esc_returns() {
    let (_temp, paths, plan) = full_plan_fixture(2);

    let text = render_plan_attach_text(&paths, &plan, &[], &[], 0);

    assert!(text.contains("Enter waits for child run"), "{text}");
    assert!(text.contains("q/Esc/Ctrl-D detach"), "{text}");
}

#[test]
fn plan_attach_footer_snapshot_captures_back_navigation_grammar() {
    let (_temp, paths, plan) = full_plan_fixture(2);

    let footer = plan_attach_footer(
        &paths,
        &plan,
        0,
        true,
        AttachViewMode::Activity,
        NarrativeVisualMode::Architecture,
    );

    assert!(footer.starts_with("q/Esc/Ctrl-D detach"), "{footer}");
    assert!(footer.contains("arrows/Tab focus child"), "{footer}");
    assert!(footer.contains("Enter waits for child run"), "{footer}");
    assert_eq!(footer.matches("recommended:").count(), 1, "{footer}");
    assert!(footer.contains("recommended: deadreckon fork"), "{footer}");
    assert!(!footer.contains("try:"), "{footer}");
}

#[test]
fn attach_plan_enter_opens_selected_child_run_detail() {
    let (temp, paths, mut plan) = full_plan_fixture(2);
    let state = create_run(
        &paths,
        RunOptions {
            goal: "child detail".to_string(),
            cwd: temp.path().to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some("smoke".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");
    plan.tasks[0].child_run_id = Some(state.run_id);

    let footer = plan_attach_footer(
        &paths,
        &plan,
        0,
        true,
        AttachViewMode::Activity,
        NarrativeVisualMode::Architecture,
    );

    assert!(footer.contains("Enter child run"), "{footer}");
    assert!(!footer.contains("try: deadreckon fork"), "{footer}");
    assert!(!footer.contains("recommended:"), "{footer}");
}

#[test]
fn attach_plan_back_returns_to_same_selected_task() {
    let (_temp, paths, plan) = full_plan_fixture(2);
    let text = render_plan_attach_text(&paths, &plan, &[], &[], 1);

    assert!(attach_should_return_to_plan(KeyEvent::new(
        KeyCode::Char('b'),
        KeyModifiers::NONE
    )));
    assert!(attach_should_return_to_plan(KeyEvent::new(
        KeyCode::Backspace,
        KeyModifiers::NONE
    )));
    assert!(text.contains("* task-1"), "{text}");
}

#[test]
fn attach_plan_enter_without_run_id_shows_recommended_footer() {
    let (_temp, paths, plan) = full_plan_fixture(2);

    let text = render_plan_attach_text(&paths, &plan, &[], &[], 0);

    assert!(text.contains("Enter waits for child run"), "{text}");
    assert!(
        text.contains(&format!(
            "recommended: deadreckon fork {}",
            &plan.plan_id[..8]
        )),
        "{text}"
    );
    assert!(!text.contains("try: deadreckon fork"), "{text}");
}

#[test]
fn attach_plan_missing_child_run_shows_one_recommended_recovery() {
    let (_temp, paths, mut plan) = full_plan_fixture(2);
    plan.tasks[0].child_run_id = Some("missing-child-run".to_string());

    let footer = plan_attach_footer(
        &paths,
        &plan,
        0,
        true,
        AttachViewMode::Activity,
        NarrativeVisualMode::Architecture,
    );

    assert!(footer.contains("child detail unavailable"), "{footer}");
    assert_eq!(footer.matches("recommended:").count(), 1, "{footer}");
    assert!(
        footer.contains("recommended: deadreckon list --all"),
        "{footer}"
    );
    assert!(!footer.contains("try:"), "{footer}");
}

#[test]
fn plan_attach_overview_breadcrumb_names_plan() {
    let (_temp, paths, plan) = full_plan_fixture(2);

    let text = render_plan_attach_text(&paths, &plan, &[], &[], 0);

    assert!(text.contains("deadreckon plan"), "{text}");
    assert!(text.contains(&plan.plan_id[..8]), "{text}");
}

#[test]
fn child_attach_from_plan_names_parent_and_back_action() {
    let (_temp, state) = doc_preview_state();
    let tui_state = AttachTuiState {
        parent_plan: Some(AttachParentPlan {
            plan_id: "99998888777766665555444433332222".to_string(),
            task_id: "task-1".to_string(),
            campaign_parent: None,
        }),
        ..AttachTuiState::default()
    };

    let text = render_attach_text_with_tui_state(&state, &[], &AttachLive::default(), tui_state);

    assert!(text.contains("plan 99998888 / task-1"), "{text}");
    assert!(
        text.contains("Back to plan: b Backspace q Esc Ctrl-D"),
        "{text}"
    );
    assert!(text.contains("parent plan 99998888 task-1"), "{text}");
}

#[test]
fn plan_attach_child_breadcrumb_names_task_and_run() {
    let (_temp, state) = doc_preview_state();
    let tui_state = AttachTuiState {
        parent_plan: Some(AttachParentPlan {
            plan_id: "99998888777766665555444433332222".to_string(),
            task_id: "task-1".to_string(),
            campaign_parent: None,
        }),
        ..AttachTuiState::default()
    };

    let text = render_attach_text_with_tui_state(&state, &[], &AttachLive::default(), tui_state);

    assert!(text.contains("plan 99998888 / task-1"), "{text}");
    assert!(
        text.contains(&format!("run {}", &state.run_id[..8])),
        "{text}"
    );
}

#[test]
fn plan_attach_child_footer_includes_back_hint() {
    let (_temp, state) = doc_preview_state();
    let tui_state = AttachTuiState {
        parent_plan: Some(AttachParentPlan {
            plan_id: "99998888777766665555444433332222".to_string(),
            task_id: "task-1".to_string(),
            campaign_parent: None,
        }),
        ..AttachTuiState::default()
    };

    let text = render_attach_text_with_tui_state(&state, &[], &AttachLive::default(), tui_state);

    assert!(
        text.contains("Back to plan: b Backspace q Esc Ctrl-D"),
        "{text}"
    );
}

#[test]
fn attach_plan_ctrl_d_detaches_does_not_kill() {
    let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);

    assert!(attach_should_quit(key));
}

#[test]
fn attach_plan_q_detaches_from_child_without_killing() {
    let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);

    assert!(attach_should_quit(key));
}

#[test]
fn malformed_plan_event_line_does_not_break_attach() {
    let (_temp, paths, plan) = full_plan_fixture(2);
    save_plan(&paths, &plan).expect("save plan");
    append_plan_event(&paths, &plan.plan_id, PlanEventKind::PlanStarted).expect("append started");
    std::fs::OpenOptions::new()
        .append(true)
        .open(paths.plan_events(&plan.plan_id))
        .expect("open events")
        .write_all(b"not-json\n")
        .expect("bad line");

    let text = render_plan_attach_text(
        &paths,
        &plan,
        &[],
        &read_plan_events_lossy(&paths, &plan.plan_id),
        0,
    );

    assert!(text.contains("plan started"), "{text}");
}

#[test]
fn tui_budget_callout_appears_above_60_percent() {
    let (_temp, mut state) = doc_preview_state();
    state.provider = Some("openai".to_string());
    state.total_spend_usd = 6.5;
    state.max_spend_usd = Some(10.0);

    let text = render_attach_text(&state, &[], &AttachLive::default());

    assert!(text.contains("$6.500000 / $10.000000"), "{text}");
    assert!(text.contains("65% of budget"), "{text}");
}

#[test]
fn polish_preview_block_lists_provider_and_subskills() {
    let (_temp, state) = doc_preview_state();
    let estimate = SpendEstimate {
        provider: "cli:codex".to_string(),
        model: "provider default".to_string(),
        input_tokens: 0,
        output_tokens: 65_536,
        cost_usd: 0.0,
        subscription: true,
        wall_time_seconds: None,
    };
    let text = doc_polish_preview_text(
        &state,
        "cli:codex",
        "auto_subscription",
        &[
            "narrator-overview".to_string(),
            "narrator-phases".to_string(),
            "narrator-as-built".to_string(),
            "narrator-decisions".to_string(),
        ],
        16_384,
        Some(0.0),
        &estimate,
    )
    .expect("preview");
    assert!(text.contains("provider:"));
    assert!(text.contains("cli:codex"));
    assert!(text.contains("narrator-overview, narrator-phases"));
    assert!(text.contains("$0.00 (subscription)"));
}

#[test]
fn polish_preview_suppressed_by_hints_env() {
    assert!(!completion_hints_enabled(true));
}

fn counts() -> AttachPanelCounts {
    AttachPanelCounts {
        activity: 20,
        files: 10,
        processes: 3,
    }
}

fn rows() -> AttachPanelRows {
    AttachPanelRows {
        activity: 5,
        files: 4,
        processes: 4,
    }
}

#[test]
fn render_chain_attach_unit_snapshot() {
    let chain = chain_fixture();
    let events = vec![chain_event_record(&chain.chain_id, 1)];

    let text = render_chain_attach_text(&chain, &events, &ChainAttachTuiState::default());

    assert!(text.contains("deadreckon chain"), "{text}");
    assert!(
        text.contains("policy branch=stack apply=auto strategy=squash on-fail=stop"),
        "{text}"
    );
    assert!(text.contains("steps"), "{text}");
    assert!(text.contains("chain activity"), "{text}");
    assert!(text.contains("applied"), "{text}");
    assert!(text.contains("running"), "{text}");
    assert!(text.contains("step started step 2"), "{text}");
}

#[test]
fn chain_attach_renders_step_timeline_with_status_dots() {
    let chain = chain_fixture();
    let tui_state = ChainAttachTuiState::default();

    let lines = chain_timeline_lines(&chain, &tui_state)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();

    assert!(lines[0].contains("◉ step  1 applied"));
    assert!(lines[0].contains("run aaaaaaaa"));
    assert!(lines[1].contains("● step  2 running"));
}

#[test]
fn large_chain_timeline_still_scrolls() {
    let chain = Chain::new(ChainNewOptions {
        root_goal: "large chain".to_string(),
        goals: (0..12)
            .map(|index| format!("step goal {index}"))
            .collect::<Vec<_>>(),
        scope: "scope".to_string(),
        base_branch: "main".to_string(),
        base_sha: "abcdef123456".to_string(),
        cwd: std::path::PathBuf::from("/tmp/project"),
        provider: Some("smoke".to_string()),
        model: None,
        sandbox: "none".to_string(),
        branch_policy: BranchPolicy::Stack,
        apply_mode: ApplyMode::Auto,
        apply_strategy: ApplyStrategy::Squash,
        apply_allowlist: Vec::new(),
        on_fail: OnFail::Stop,
        circuit_breaker_threshold: 2,
        max_spend_usd: Some(5.0),
        max_wall_seconds: Some(600.0),
        deadreckon_version: "0.1.0".to_string(),
    })
    .expect("chain");
    let mut tui_state = ChainAttachTuiState::default();

    for _ in 0..15 {
        tui_state.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()), &chain);
    }
    let lines = chain_timeline_lines(&chain, &tui_state)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();

    assert_eq!(tui_state.selected_step, 11);
    assert!(lines[11].contains(">"));
    assert!(lines[11].contains("step 12"));

    tui_state.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::empty()), &chain);

    assert_eq!(tui_state.selected_step, 11);
}

#[test]
fn chain_attach_header_shows_policy_apply_mode_on_fail() {
    let chain = chain_fixture();
    let header = chain_attach_header_text(&chain);

    assert!(header.contains("status pending"));
    assert!(header.contains("policy branch=stack apply=auto strategy=squash on-fail=stop"));
    assert!(header.contains("spend $0.000000/$5.000000"));
}

#[test]
fn chain_attach_activity_lists_newest_events_first() {
    let chain = chain_fixture();
    let events = vec![
        ChainEvent {
            timestamp: Utc::now(),
            chain_id: chain.chain_id.clone(),
            event: ChainEventKind::ChainCreated,
            step_index: None,
            detail: serde_json::json!({ "goal": "build app" }),
        },
        ChainEvent {
            timestamp: Utc::now(),
            chain_id: chain.chain_id.clone(),
            event: ChainEventKind::ChainStepStarted,
            step_index: Some(1),
            detail: serde_json::json!({ "goal": "second step" }),
        },
    ];

    let lines = chain_activity_lines(&events, &ChainAttachTuiState::default())
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();

    assert!(lines[0].contains("step started step 2"));
    assert!(lines[1].contains("created"));
}

#[test]
fn chain_attach_paused_footer_lists_one_recommended_command() {
    let mut chain = chain_fixture();
    chain.status = ChainStatus::Paused;
    chain.paused_reason = Some("apply_refused_conflict".to_string());
    let id = super::run_prefix(&chain.chain_id);

    let footer = chain_attach_footer_text(&chain);

    assert!(footer.contains("paused: apply_refused_conflict"));
    assert_eq!(footer.matches("try:").count(), 0, "{footer}");
    assert_eq!(footer.matches("recommended:").count(), 1, "{footer}");
    assert!(
        footer.contains(&format!("recommended: deadreckon chain resume {id}")),
        "{footer}"
    );
    assert!(
        footer.contains(&format!("deadreckon chain show {id} --why-failed")),
        "{footer}"
    );
    assert!(
        footer.contains(&format!(
            "deadreckon chain resume {id} --apply-mode preview"
        )),
        "{footer}"
    );
    assert!(
        footer.contains(&format!("deadreckon chain undo {id}")),
        "{footer}"
    );
}

#[test]
fn chain_default_auto_attaches_when_stdout_tty() {
    assert!(chain_should_auto_attach(true, false, false, false));
    assert!(!chain_should_auto_attach(false, false, false, false));
    assert!(!chain_should_auto_attach(true, true, false, false));
    assert!(!chain_should_auto_attach(true, false, true, false));
    assert!(!chain_should_auto_attach(true, false, false, true));
}

#[test]
fn chain_attach_budget_bar_thresholds_60_80_percent() {
    assert_eq!(threshold_color(0.59), Color::Green);
    assert_eq!(threshold_color(0.60), Color::Yellow);
    assert_eq!(threshold_color(0.79), Color::Yellow);
    assert_eq!(threshold_color(0.80), Color::Red);
}

#[test]
fn spend_gauge_uses_gradient_and_pause_cap_palette() {
    let (_temp, mut state) = doc_preview_state();

    assert_eq!(meter_color(0.30, &state), Color::Green);
    assert_eq!(meter_color(0.70, &state), Color::Yellow);
    assert_eq!(meter_color(0.90, &state), Color::Red);

    state.pause_reason = Some("spend cap reached".to_string());
    assert_eq!(meter_color(0.90, &state), Color::Magenta);
}

#[test]
fn deadreckoning_course_animation_moves() {
    let first = deadreckoning_course_ascii(16, 0);
    let second = deadreckoning_course_ascii(16, 1);

    assert_ne!(first, second);
    assert!(first.contains('*'));
    assert_eq!(first.chars().count(), 16);
}

#[test]
fn deadreckoning_course_strip_matches_identity_golden() {
    assert_eq!(deadreckoning_course_ascii(18, 0), "*--.--.^-.--.-^.--");
}

#[test]
fn chain_step_glyphs_match_identity_set() {
    assert_eq!(chain_step_dot(ChainStepStatus::Pending), "○");
    assert_eq!(chain_step_dot(ChainStepStatus::Running), "●");
    assert_eq!(chain_step_dot(ChainStepStatus::Completed), "◐");
    assert_eq!(chain_step_dot(ChainStepStatus::Failed), "✗");
    assert_eq!(chain_step_dot(ChainStepStatus::Skipped), "↷");
    assert_eq!(chain_step_dot(ChainStepStatus::Applied), "◉");
    assert_eq!(chain_step_dot(ChainStepStatus::Undone), "↶");
}

#[test]
fn attach_footer_status_names_running_state() {
    let (_temp, mut state) = doc_preview_state();
    state.status = deadreckon_core::RunStatus::Executing;
    state.current_phase_id = deadreckon_core::PhaseId(40);

    let text = deadreckoning_status_text(&state, "42s running", 100, 3);

    assert!(text.contains("deadreckoning running"), "{text}");
    assert!(text.contains("turn 42s running"), "{text}");
    assert!(text.contains("execute"), "{text}");
    assert!(text.contains('*'), "{text}");
}

#[test]
fn attach_header_is_identity_strip_without_live_status() {
    let (_temp, mut state) = doc_preview_state();
    state.status = deadreckon_core::RunStatus::Executing;
    state.current_phase_id = deadreckon_core::PhaseId(40);

    let text = attach_header_text(&state, 96);

    assert!(text.contains("run "), "{text}");
    assert!(text.contains("provider cli:codex"), "{text}");
    assert!(text.contains("sandbox none"), "{text}");
    assert!(text.contains("goal preview docs"), "{text}");
    assert!(text.contains("working "), "{text}");
    assert!(!text.contains("status executing"), "{text}");
    assert!(!text.contains("phase 40"), "{text}");
    assert!(!text.contains("turn "), "{text}");
}

#[test]
fn acceptance_activity_lines_surface_running_and_failed_checks() {
    let acceptance = AcceptanceLive {
        status: AcceptanceUiStatus::Failed,
        total: 3,
        completed: 2,
        passed: 1,
        failed: 1,
        required_failed: 1,
        latest_detail: Some("npm test exited with status 1".to_string()),
        progress_lines: vec![
            "✗ shell npm test exited with status 1".to_string(),
            "✓ file_exists package.json exists".to_string(),
        ],
    };

    let lines = acceptance_activity_lines(&acceptance).join("\n");

    assert!(lines.contains("acceptance failed"), "{lines}");
    assert!(lines.contains("1 required failures"), "{lines}");
    assert!(lines.contains("npm test"), "{lines}");
}

fn test_log_spec(cwd_match: IngestCwdMatch) -> ProviderJsonlLogSpec {
    ProviderJsonlLogSpec {
        schema: "test".to_string(),
        roots: Vec::new(),
        since: Utc::now(),
        cwd_match,
        cwd_match_path: None,
        storage: IngestStorage::Jsonl,
        file_glob: Some("*.jsonl".to_string()),
    }
}

#[test]
fn provider_log_spec_uses_descriptor_roots_for_codex() {
    let (_temp, state) = doc_preview_state();
    let registry = ProviderRegistry::builtin().expect("registry");
    let home = std::path::PathBuf::from("/tmp/deadreckon-home");

    let spec =
        provider_jsonl_log_spec_from_registry(&state, &registry, &home).expect("codex log spec");

    assert_eq!(spec.schema, "codex-cli");
    assert_eq!(spec.cwd_match, IngestCwdMatch::SessionMeta);
    assert!(spec.roots.contains(&home.join(".codex/sessions")));
    assert!(spec.roots.contains(&home.join(".codex/archived_sessions")));
}

#[test]
fn provider_log_spec_honors_ingest_env_override() {
    let home = std::path::PathBuf::from("/tmp/deadreckon-home");
    let first = std::path::PathBuf::from("/tmp/codex-one");
    let second = std::path::PathBuf::from("/tmp/codex-two");
    let env_value =
        std::env::join_paths([first.as_path(), second.as_path()]).expect("join env paths");
    let ingest = IngestDescriptor {
        env_var: Some("CODEX_SESSIONS_DIR".to_string()),
        default_dirs: vec![std::path::PathBuf::from("~/.codex/sessions")],
        ..IngestDescriptor::default()
    };

    let roots = provider_ingest_base_roots(&ingest, &home, Some(env_value.as_os_str()));

    assert_eq!(roots, [first, second]);
}

#[test]
fn claude_ingest_roots_remain_workdir_scoped_and_deduped() {
    let (_temp, mut state) = doc_preview_state();
    state.provider = Some("cli:claude-code".to_string());
    let registry = ProviderRegistry::builtin().expect("registry");
    let home = std::path::PathBuf::from("/tmp/deadreckon-home");

    let spec =
        provider_jsonl_log_spec_from_registry(&state, &registry, &home).expect("claude log spec");
    let expected = home
        .join(".claude/projects")
        .join(claude_project_name_for_workdir(
            &state.working_dir.to_string_lossy(),
        ));

    assert_eq!(spec.schema, "claude-code");
    assert_eq!(spec.cwd_match, IngestCwdMatch::ClaudeProjectDir);
    assert!(spec.roots.contains(&expected), "{:?}", spec.roots);
    let mut deduped = spec.roots.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(spec.roots, deduped);
}

#[test]
fn provider_jsonl_matchers_cover_session_meta_and_top_level_cwd() {
    let temp = test_tempdir();
    let working_dir = temp.path().join("work");
    let working_dirs = vec![working_dir.to_string_lossy().to_string()];

    let codex = temp.path().join("codex.jsonl");
    std::fs::write(
        &codex,
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\"}}}}\n",
            working_dir.display()
        ),
    )
    .expect("codex jsonl");
    let codex_spec = test_log_spec(IngestCwdMatch::SessionMeta);
    assert!(provider_jsonl_session_matches_run(
        &codex_spec,
        &codex,
        &working_dirs
    ));

    let claude = temp.path().join("claude.jsonl");
    std::fs::write(
        &claude,
        format!(
            "{{\"type\":\"assistant\",\"cwd\":\"{}\",\"message\":{{\"content\":[]}}}}\n",
            working_dir.display()
        ),
    )
    .expect("claude jsonl");
    let claude_spec = test_log_spec(IngestCwdMatch::TopLevel);
    assert!(provider_jsonl_session_matches_run(
        &claude_spec,
        &claude,
        &working_dirs
    ));
}

#[test]
fn cwd_match_directory_field_matches_opencode_json() {
    let temp = test_tempdir();
    let working_dir = temp.path().join("work");
    let working_dirs = vec![working_dir.to_string_lossy().to_string()];
    let path = temp.path().join("opencode.json");
    std::fs::write(
        &path,
        format!(r#"{{"id":"s1","directory":"{}"}}"#, working_dir.display()),
    )
    .expect("opencode json");

    let mut spec = test_log_spec(IngestCwdMatch::DirectoryField);
    spec.storage = IngestStorage::Json;

    assert!(provider_jsonl_session_matches_run(
        &spec,
        &path,
        &working_dirs
    ));
}

#[test]
fn provider_jsonl_activity_dispatches_codex_and_claude_rows() {
    let mut codex = ProviderActivity::default();
    let codex_lines = provider_jsonl_activity_lines(
        "codex-cli",
        r#"{"type":"event_msg","timestamp":"2026-05-13T02:34:17Z","payload":{"type":"agent_message","message":"Working on it"}}"#,
        &mut codex,
    );
    assert_eq!(codex_lines.len(), 1);
    assert!(codex_lines[0].contains("agent Working on it"));
    let codex_tool_lines = provider_jsonl_activity_lines(
        "codex-cli",
        r#"{"type":"response_item","timestamp":"2026-05-13T02:34:18Z","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"cargo test\"}"}}"#,
        &mut codex,
    );
    assert_eq!(codex_tool_lines.len(), 1);
    assert!(codex_tool_lines[0].contains("tool Bash cargo test"));

    let mut claude = ProviderActivity::default();
    let claude_lines = provider_jsonl_activity_lines(
        "claude-code",
        r#"{"type":"assistant","timestamp":"2026-05-13T02:34:17.615Z","message":{"usage":{"input_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":4},"content":[{"type":"text","text":"Adding tests"},{"type":"tool_use","name":"Bash","input":{"command":"npm test"}}]}}"#,
        &mut claude,
    );
    assert_eq!(claude.context_tokens, Some(10));
    assert_eq!(claude.context_window, Some(200_000));
    assert_eq!(claude_lines.len(), 2);
    assert!(claude_lines[0].contains("agent Adding tests"));
    assert!(claude_lines[1].contains("tool Bash npm test"));
}

#[test]
fn provider_jsonl_copilot_activity_parses_assistant_message_and_usage() {
    let mut activity = ProviderActivity::default();
    let lines = provider_jsonl_activity_lines(
        "copilot-cli",
        r#"{"type":"assistant.message","timestamp":"2026-05-13T02:34:17Z","usage":{"inputTokens":10,"output_tokens":4,"cacheReadTokens":2,"cacheWriteTokens":1},"data":{"reasoningText":"Need a plan","content":"I will edit the file","outputTokens":4}}"#,
        &mut activity,
    );
    let joined = lines.join("\n");

    assert!(
        joined.contains("tokens input 10 output 4 cache 3"),
        "{joined}"
    );
    assert!(joined.contains("thinking Need a plan"), "{joined}");
    assert!(joined.contains("agent I will edit the file"), "{joined}");
    assert!(joined.contains("tokens output 4"), "{joined}");
    assert_eq!(activity.context_tokens, Some(13));
    assert_eq!(activity.context_window, Some(258_400));
}

#[test]
fn provider_jsonl_copilot_activity_parses_tool_request_and_result() {
    let mut activity = ProviderActivity::default();
    let tool_lines = provider_jsonl_activity_lines(
        "copilot-cli",
        r#"{"type":"assistant.message","timestamp":"2026-05-13T02:34:18Z","data":{"toolRequests":[{"toolCallId":"t1","name":"bash","arguments":{"command":"cargo test"}}]}}"#,
        &mut activity,
    );
    let result_lines = provider_jsonl_activity_lines(
        "copilot-cli",
        r#"{"type":"tool.execution_complete","timestamp":"2026-05-13T02:34:19Z","data":{"toolCallId":"t1","result":"tests passed"}}"#,
        &mut activity,
    );

    assert!(tool_lines.join("\n").contains("tool Bash cargo test"));
    assert!(result_lines.join("\n").contains("result tests passed"));
}

#[test]
fn provider_jsonl_copilot_activity_ignores_unrelated_event_rows() {
    let mut activity = ProviderActivity::default();
    let lines = provider_jsonl_activity_lines(
        "copilot-cli",
        r#"{"type":"session.start","timestamp":"2026-05-13T02:34:16Z","data":{"sessionId":"s1"}}"#,
        &mut activity,
    );

    assert!(lines.is_empty());
}

#[test]
fn provider_jsonl_pi_activity_parses_text_thinking_tool_and_result_blocks() {
    let mut activity = ProviderActivity::default();
    let assistant_lines = provider_jsonl_activity_lines(
        "pi",
        r#"{"type":"message","timestamp":"2026-05-13T02:34:17Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Need to inspect"},{"type":"text","text":"I will inspect the file"},{"type":"toolCall","id":"t1","name":"bash","arguments":{"command":"cargo test"}}]}}"#,
        &mut activity,
    );
    let result_lines = provider_jsonl_activity_lines(
        "pi",
        r#"{"type":"message","timestamp":"2026-05-13T02:34:18Z","message":{"role":"toolResult","toolCallId":"t1","content":"ok"}}"#,
        &mut activity,
    );
    let joined = assistant_lines.join("\n");

    assert!(joined.contains("thinking Need to inspect"), "{joined}");
    assert!(joined.contains("agent I will inspect the file"), "{joined}");
    assert!(joined.contains("tool Bash cargo test"), "{joined}");
    assert!(result_lines.join("\n").contains("result ok"));
}

#[test]
fn provider_jsonl_pi_activity_normalizes_intent_argument_description() {
    let mut activity = ProviderActivity::default();
    let lines = provider_jsonl_activity_lines(
        "pi",
        r#"{"type":"message","timestamp":"2026-05-13T02:34:17Z","message":{"role":"assistant","content":[{"type":"toolCall","name":"Task","arguments":{"agent__intent":"review docs","prompt":"read files"}}]}}"#,
        &mut activity,
    );

    assert!(lines.join("\n").contains(r#""description":"review docs""#));
}

#[test]
fn provider_jsonl_pi_activity_extracts_usage_context_tokens() {
    let mut activity = ProviderActivity::default();
    let lines = provider_jsonl_activity_lines(
        "pi",
        r#"{"type":"message","timestamp":"2026-05-13T02:34:17Z","message":{"role":"assistant","usage":{"input":10,"output":5,"cache":{"read":3,"write":2}},"content":"Done"}}"#,
        &mut activity,
    );
    let joined = lines.join("\n");

    assert!(
        joined.contains("tokens input 10 output 5 cache 5"),
        "{joined}"
    );
    assert!(joined.contains("agent Done"), "{joined}");
    assert_eq!(activity.context_tokens, Some(15));
    assert_eq!(activity.context_window, Some(1_000_000));
}

#[test]
fn schema_dispatch_unknown_schema_is_quiet() {
    let mut activity = ProviderActivity::default();
    let lines = provider_jsonl_activity_lines(
        "unknown-schema",
        r#"{"type":"event_msg","payload":{"type":"agent_message","message":"hidden"}}"#,
        &mut activity,
    );

    assert!(lines.is_empty());
}

#[test]
fn provider_jsonl_copilot_ingest_discovers_bare_session_state_jsonl() {
    let temp = test_tempdir();
    let (_state_temp, mut state) = doc_preview_state();
    state.provider = Some("cli:copilot".to_string());
    let home = temp.path().join("home");
    let session_dir = home.join(".copilot/session-state");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    std::fs::write(
        session_dir.join("abc.jsonl"),
        format!(
            r#"{{"type":"session.start","timestamp":"2026-05-13T02:34:16Z","data":{{"context":{{"cwd":"{}"}}}}}}
{{"type":"assistant.message","timestamp":"2026-05-13T02:34:17Z","data":{{"content":"Copilot edited the file"}}}}
"#,
            state.working_dir.display()
        ),
    )
    .expect("copilot session");
    let registry = ProviderRegistry::builtin().expect("registry");
    let spec =
        provider_jsonl_log_spec_from_registry(&state, &registry, &home).expect("copilot spec");

    let activity = collect_jsonl_provider_activity(&state, &spec);
    let lines = activity.lines.join("\n");

    assert!(lines.contains("agent Copilot edited the file"), "{lines}");
    assert!(lines.contains("provider log"), "{lines}");
}

#[test]
fn provider_jsonl_copilot_ingest_discovers_nested_events_jsonl() {
    let temp = test_tempdir();
    let (_state_temp, mut state) = doc_preview_state();
    state.provider = Some("cli:copilot".to_string());
    let home = temp.path().join("home");
    let session_dir = home.join(".copilot/session-state/sess-1");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    std::fs::write(
        session_dir.join("events.jsonl"),
        format!(
            r#"{{"type":"session.start","timestamp":"2026-05-13T02:34:16Z","data":{{"context":{{"cwd":"{}"}}}}}}
{{"type":"assistant.message","timestamp":"2026-05-13T02:34:17Z","data":{{"content":"Nested event worked"}}}}
"#,
            state.working_dir.display()
        ),
    )
    .expect("events");
    let registry = ProviderRegistry::builtin().expect("registry");
    let spec =
        provider_jsonl_log_spec_from_registry(&state, &registry, &home).expect("copilot spec");

    let activity = collect_jsonl_provider_activity(&state, &spec);

    assert!(activity.lines.join("\n").contains("Nested event worked"));
}

#[test]
fn provider_jsonl_copilot_ingest_json_pointer_cwd_matches_session_start() {
    let temp = test_tempdir();
    let working_dir = temp.path().join("work");
    let path = temp.path().join("copilot.jsonl");
    std::fs::write(
        &path,
        format!(
            r#"{{"type":"session.start","data":{{"context":{{"cwd":"{}"}}}}}}"#,
            working_dir.display()
        ),
    )
    .expect("copilot jsonl");
    let mut spec = test_log_spec(IngestCwdMatch::JsonPointer);
    spec.cwd_match_path = Some("data.context.cwd".to_string());

    assert!(provider_jsonl_session_matches_run(
        &spec,
        &path,
        &[working_dir.to_string_lossy().to_string()]
    ));
}

#[test]
fn provider_jsonl_pi_ingest_discovers_session_jsonl_under_encoded_cwd_dir() {
    let temp = test_tempdir();
    let (_state_temp, mut state) = doc_preview_state();
    state.provider = Some("cli:pi".to_string());
    let home = temp.path().join("home");
    let session_dir = home.join(".pi/agent/sessions/--tmp-work--");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    std::fs::write(
        session_dir.join("pi-session.jsonl"),
        format!(
            r#"{{"type":"session","id":"s1","timestamp":"2026-05-13T02:34:16Z","cwd":"{}"}}
{{"type":"message","timestamp":"2026-05-13T02:34:17Z","message":{{"role":"assistant","content":"Pi edited the file"}}}}
"#,
            state.working_dir.display()
        ),
    )
    .expect("pi session");
    let registry = ProviderRegistry::builtin().expect("registry");
    let spec = provider_jsonl_log_spec_from_registry(&state, &registry, &home).expect("pi spec");

    let activity = collect_jsonl_provider_activity(&state, &spec);
    let lines = activity.lines.join("\n");

    assert!(lines.contains("agent Pi edited the file"), "{lines}");
    assert!(lines.contains("provider log"), "{lines}");
}

#[test]
fn provider_jsonl_pi_ingest_rejects_jsonl_without_session_header() {
    let temp = test_tempdir();
    let (_state_temp, mut state) = doc_preview_state();
    state.provider = Some("cli:pi".to_string());
    let home = temp.path().join("home");
    let session_dir = home.join(".pi/agent/sessions/--tmp-work--");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    std::fs::write(
        session_dir.join("not-pi.jsonl"),
        format!(
            r#"{{"type":"note","cwd":"{}"}}
{{"type":"message","timestamp":"2026-05-13T02:34:17Z","message":{{"role":"assistant","content":"Should not show"}}}}
"#,
            state.working_dir.display()
        ),
    )
    .expect("not pi");
    let registry = ProviderRegistry::builtin().expect("registry");
    let spec = provider_jsonl_log_spec_from_registry(&state, &registry, &home).expect("pi spec");

    let activity = collect_jsonl_provider_activity(&state, &spec);

    assert!(activity.lines.is_empty(), "{:?}", activity.lines);
}

#[test]
fn provider_jsonl_pi_ingest_top_level_cwd_matches_session_header() {
    let temp = test_tempdir();
    let working_dir = temp.path().join("work");
    let path = temp.path().join("pi.jsonl");
    std::fs::write(
        &path,
        format!(
            r#"{{"type":"session","id":"s1","cwd":"{}"}}"#,
            working_dir.display()
        ),
    )
    .expect("pi jsonl");
    let spec = test_log_spec(IngestCwdMatch::TopLevel);

    assert!(provider_jsonl_session_matches_run(
        &spec,
        &path,
        &[working_dir.to_string_lossy().to_string()]
    ));
}

#[test]
fn gemini_json_object_fixture_emits_agent_tool_result_and_tokens() {
    let temp = test_tempdir();
    let (_state_temp, state) = doc_preview_state();
    let root = temp.path().join("gemini");
    std::fs::create_dir_all(&root).expect("gemini root");
    std::fs::write(
        root.join("session-test.json"),
        r#"{
  "sessionId": "s1",
  "messages": [{
"type": "gemini",
"timestamp": "2026-05-13T02:34:17Z",
"thoughts": [{"subject": "Plan", "description": "Read the file"}],
"content": "I will inspect the file",
"tokens": {"input": 10, "cached": 2, "output": 3},
"toolCalls": [{
  "name": "read_file",
  "args": {"path": "src/main.rs"},
  "result": [{"functionResponse": {"id": "r1", "response": {"output": "file contents"}}}]
}]
  }]
}"#,
    )
    .expect("gemini fixture");
    let spec = ProviderJsonlLogSpec {
        schema: "gemini".to_string(),
        roots: vec![root],
        since: Utc::now() - chrono::Duration::minutes(1),
        cwd_match: IngestCwdMatch::None,
        cwd_match_path: None,
        storage: IngestStorage::JsonOrJsonl,
        file_glob: None,
    };

    let activity = collect_jsonl_provider_activity(&state, &spec);
    let lines = activity.lines.join("\n");

    assert!(lines.contains("thinking Plan Read the file"), "{lines}");
    assert!(lines.contains("agent I will inspect the file"), "{lines}");
    assert!(lines.contains("tool Read src/main.rs"), "{lines}");
    assert!(lines.contains("result file contents"), "{lines}");
    assert_eq!(activity.context_tokens, Some(12));
    assert_eq!(activity.context_window, Some(1_000_000));
}

#[test]
fn gemini_jsonl_fixture_emits_activity_and_tokens() {
    let temp = test_tempdir();
    let (_state_temp, state) = doc_preview_state();
    let root = temp.path().join("gemini");
    std::fs::create_dir_all(&root).expect("gemini root");
    std::fs::write(
        root.join("session-test.jsonl"),
        r#"{"type":"user","id":"u1","timestamp":"2026-05-13T02:34:16Z","content":"hello"}
{"type":"gemini","id":"g1","timestamp":"2026-05-13T02:34:17Z","content":[{"text":"Done"}],"tokens":{"input":4,"cached":1},"toolCalls":[{"name":"run_command","args":{"command":"cargo test"}}]}
"#,
    )
    .expect("gemini jsonl fixture");
    let spec = ProviderJsonlLogSpec {
        schema: "gemini".to_string(),
        roots: vec![root],
        since: Utc::now() - chrono::Duration::minutes(1),
        cwd_match: IngestCwdMatch::None,
        cwd_match_path: None,
        storage: IngestStorage::JsonOrJsonl,
        file_glob: None,
    };

    let activity = collect_jsonl_provider_activity(&state, &spec);
    let lines = activity.lines.join("\n");

    assert!(lines.contains("agent Done"), "{lines}");
    assert!(lines.contains("tool Bash cargo test"), "{lines}");
    assert_eq!(activity.context_tokens, Some(5));
    assert_eq!(activity.context_window, Some(1_000_000));
}

#[test]
fn opencode_storage_fixture_emits_agent_thinking_tool_and_tokens() {
    let temp = test_tempdir();
    let (_state_temp, state) = doc_preview_state();
    let root = temp.path().join("opencode");
    let session_dir = root.join("storage/session/project");
    let message_dir = root.join("storage/message/s1");
    let part_dir = root.join("storage/part/m1");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    std::fs::create_dir_all(&message_dir).expect("message dir");
    std::fs::create_dir_all(&part_dir).expect("part dir");
    std::fs::write(
        session_dir.join("s1.json"),
        format!(
            r#"{{"id":"s1","directory":"{}","time":{{"created":1770000000000}}}}"#,
            state.working_dir.display()
        ),
    )
    .expect("session");
    std::fs::write(
        message_dir.join("m1.json"),
        r#"{"id":"m1","sessionID":"s1","role":"assistant","time":{"created":1770000000000}}"#,
    )
    .expect("message");
    std::fs::write(
        part_dir.join("01.json"),
        r#"{"id":"p1","messageID":"m1","type":"reasoning","content":"Need to edit","time":{"created":1770000000001}}"#,
    )
    .expect("reasoning");
    std::fs::write(
        part_dir.join("02.json"),
        r#"{"id":"p2","messageID":"m1","type":"text","content":"Editing now","time":{"created":1770000000002}}"#,
    )
    .expect("text");
    std::fs::write(
        part_dir.join("03.json"),
        r#"{"id":"p3","messageID":"m1","type":"tool","tool":"bash","state":{"input":{"command":"cargo test"}},"time":{"created":1770000000003}}"#,
    )
    .expect("tool");
    std::fs::write(
        part_dir.join("04.json"),
        r#"{"id":"p4","messageID":"m1","type":"step-finish","tokens":{"input":7,"cache":{"read":2,"write":1}},"time":{"created":1770000000004}}"#,
    )
    .expect("tokens");
    let spec = ProviderJsonlLogSpec {
        schema: "opencode".to_string(),
        roots: vec![root],
        since: Utc::now() - chrono::Duration::minutes(1),
        cwd_match: IngestCwdMatch::DirectoryField,
        cwd_match_path: None,
        storage: IngestStorage::OpenCodeStorage,
        file_glob: Some("*.json".to_string()),
    };

    let activity = collect_jsonl_provider_activity(&state, &spec);
    let lines = activity.lines.join("\n");

    assert!(lines.contains("thinking Need to edit"), "{lines}");
    assert!(lines.contains("agent Editing now"), "{lines}");
    assert!(lines.contains("tool Bash cargo test"), "{lines}");
    assert_eq!(activity.context_tokens, Some(10));
}

#[test]
fn cli_wait_status_mentions_work_and_elapsed_seconds() {
    let text = cli_wait_status_line(
        "compiling done criteria",
        std::time::Duration::from_secs(7),
        2,
    );

    assert!(text.contains("deadreckoning"), "{text}");
    assert!(text.contains("compiling done criteria"), "{text}");
    assert!(text.contains("7s"), "{text}");
}

#[test]
fn chain_attach_shows_aggregate_spend_in_header() {
    let mut chain = chain_fixture();
    chain.total_spend_usd = 1.25;
    let header = chain_attach_header_text(&chain);

    assert!(header.contains("spend $1.250000/$5.000000"), "{header}");
}

#[test]
fn chain_attach_focused_step_streams_provider_activity() {
    let chain = chain_fixture();
    let events = vec![ChainEvent {
        timestamp: Utc::now(),
        chain_id: chain.chain_id.clone(),
        event: ChainEventKind::ChainRunCompleted,
        step_index: Some(1),
        detail: serde_json::json!({ "run_id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "status": "completed" }),
    }];

    let lines = chain_activity_lines(&events, &ChainAttachTuiState::default())
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();

    assert!(lines[0].contains("run completed step 2"), "{lines:?}");
    assert!(lines[0].contains("bbbbbbbb"), "{lines:?}");
}

#[test]
fn chain_attach_tab_pages_focus_between_steps() {
    let chain = chain_fixture();
    let mut tui_state = ChainAttachTuiState::default();

    tui_state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &chain);
    assert_eq!(tui_state.selected_step, 1);
}

#[test]
fn chain_attach_ctrl_d_detaches_does_not_kill_conductor() {
    let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);

    assert!(attach_should_quit(key));
}

#[test]
fn chain_attach_enter_drills_to_single_run_tui_esc_returns() {
    let footer = chain_attach_footer_text(&chain_fixture());

    assert!(footer.contains("[Enter] drill"));
    assert!(footer.contains("detach"));
}

#[test]
fn chain_attach_r_invokes_redo_with_confirm() {
    assert!(chain_attach_footer_text(&chain_fixture()).contains("[r] redo"));
}

#[test]
fn chain_attach_e_invokes_extend_with_prompt() {
    assert!(chain_attach_footer_text(&chain_fixture()).contains("[e] extend"));
}

#[test]
fn chain_attach_p_pauses_chain() {
    assert!(chain_attach_footer_text(&chain_fixture()).contains("[p] pause"));
}

#[test]
fn chain_attach_k_kills_chain_with_confirm() {
    assert!(chain_attach_footer_text(&chain_fixture()).contains("[k] kill"));
}

#[test]
fn chain_attach_plain_emits_periodic_snapshot_no_ansi() {
    let snapshot = chain_attach_header_text(&chain_fixture());
    let ansi_start = format!("{}[", char::from(27));

    assert!(!snapshot.contains(&ansi_start), "{snapshot}");
    assert!(snapshot.contains("policy branch=stack"), "{snapshot}");
}

#[test]
fn chain_attach_paused_footer_does_not_list_peer_try_lines() {
    let mut chain = chain_fixture();
    chain.status = ChainStatus::Paused;
    chain.paused_reason = Some("cap".to_string());

    let footer = chain_attach_footer_text(&chain);

    assert_eq!(footer.matches("try:").count(), 0, "{footer}");
    assert_eq!(footer.matches("recommended:").count(), 1, "{footer}");
    assert!(footer.contains("secondary:"), "{footer}");
}

#[test]
fn chain_wall_clock_cap_pauses_chain() {
    let mut chain = chain_fixture();
    chain.max_wall_seconds = Some(10.0);
    chain.total_wall_seconds = 10.0;

    assert!(chain_wall_cap_hit(&chain));
}

#[test]
fn chain_per_step_wall_cap_is_remaining_over_remaining_steps() {
    let mut chain = chain_fixture();
    chain.steps[0].status = ChainStepStatus::Pending;
    chain.steps[1].status = ChainStepStatus::Pending;
    chain.max_wall_seconds = Some(12.0);
    chain.total_wall_seconds = 2.0;

    assert_eq!(per_step_wall_cap(&chain, 0), Some(5.0));
}

#[test]
fn tui_scroll_offsets_clamp_to_panel_content() {
    let mut state = AttachTuiState::default();
    state.scroll_focused(100, counts(), rows());
    assert_eq!(state.activity_scroll, 15);

    state.scroll_focused(-100, counts(), rows());
    assert_eq!(state.activity_scroll, 0);

    state.focused_panel = AttachPanel::Processes;
    state.scroll_focused(10, counts(), rows());
    assert_eq!(state.processes_scroll, 0);
}

#[test]
fn tui_focus_and_page_keys_move_active_panel_only() {
    let mut state = AttachTuiState::default();
    state.handle_key(
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        counts(),
        rows(),
    );
    assert_eq!(state.focused_panel, AttachPanel::Files);

    state.handle_key(
        KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
        counts(),
        rows(),
    );
    assert_eq!(state.files_scroll, 3);
    assert_eq!(state.activity_scroll, 0);

    state.handle_key(
        KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
        counts(),
        rows(),
    );
    assert_eq!(
        state.files_scroll,
        max_panel_scroll(AttachPanel::Files, counts(), rows())
    );

    state.handle_key(
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
        counts(),
        rows(),
    );
    assert_eq!(state.focused_panel, AttachPanel::Activity);
}

#[test]
fn completion_action_parser_accepts_short_and_long_forms() {
    assert_eq!(
        completion_action_from_input("m"),
        Some(CompletionAction::Materialize)
    );
    assert_eq!(
        completion_action_from_input("export"),
        Some(CompletionAction::Materialize)
    );
    assert_eq!(
        completion_action_from_input("extend"),
        Some(CompletionAction::Extend)
    );
    assert_eq!(
        completion_action_from_input("S"),
        Some(CompletionAction::Show)
    );
    assert_eq!(
        completion_action_from_input(""),
        Some(CompletionAction::Quit)
    );
    assert_eq!(completion_action_from_input("wat"), None);
}

#[test]
fn markdown_renderer_styles_headings_lists_and_code() {
    let lines = markdown_to_tui_lines(
        "# Summary\n\nImplemented `apply`.\n\n- safer checkout\n\n```rust\nfn main() {}\n```\n",
    );
    let joined = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("Summary"), "{joined}");
    assert!(joined.contains("Implemented apply."), "{joined}");
    assert!(joined.contains("- safer checkout"), "{joined}");
    assert!(joined.contains("fn main() {}"), "{joined}");
    assert!(
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .any(|span| span.content.as_ref() == "apply"
                && span.style.fg == Some(Color::Yellow)
                && span.style.add_modifier.contains(Modifier::BOLD)),
        "inline code should keep its styling"
    );
}

#[test]
fn docs_toggle_uses_activity_panel_scroll_slot() {
    let mut state = AttachTuiState::default();
    state.toggle_docs();
    state.scroll_focused(4, counts(), rows());
    assert!(state.docs_open);
    assert_eq!(state.docs_scroll, 4);
    assert_eq!(state.activity_scroll, 0);
}

#[test]
fn post_completion_action_resets_docs_view_and_explains_next_step() {
    let mut state = AttachTuiState {
        docs_open: true,
        docs_scroll: 42,
        activity_scroll: 10,
        files_scroll: 3,
        processes_scroll: 2,
        ..AttachTuiState::default()
    };

    state.record_post_action(AttachActionNotice {
        action: CompletionAction::Apply,
        success: true,
    });

    assert!(!state.docs_open);
    assert_eq!(state.focused_panel, AttachPanel::Activity);
    assert_eq!(state.activity_scroll, 0);
    assert_eq!(state.docs_scroll, 0);
    assert_eq!(state.files_scroll, 0);
    assert_eq!(state.processes_scroll, 0);
    let notice = state
        .post_action_notice
        .as_ref()
        .expect("post-action notice")
        .lines()
        .join("\n");
    assert!(notice.contains("apply action finished"), "{notice}");
    assert!(notice.contains("q detach"), "{notice}");
}

#[test]
fn live_files_explain_cleaned_worktree() {
    let live = AttachLive {
        working_dir_exists: false,
        ..AttachLive::default()
    };

    assert_eq!(
        live_file_lines(&live),
        vec!["working tree was removed after cleanup".to_string()]
    );
}
