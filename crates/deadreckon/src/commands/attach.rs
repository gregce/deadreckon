use super::super::*;
use super::attach_runtime::*;
use crate::tui::navigation::{HelpKeyAction, handle_help_key};
use crate::tui::panes::voyage::VoyageZoomState;
use crate::tui::surfaces::chain::{chain_narrative_json_text, chain_narrative_plain_text};
use crate::tui::tree::NodeId;
use crate::tui::{
    AttachActionNotice, AttachHelpMode, AttachParentPlan, AttachTuiState, MotionPolicy,
    RunCommandModeAction, RunNarrativeRenderInput, attach_panel_counts,
    build_run_narrative_projection, render_help_overlay, run_narrative_projection,
    toggle_attach_view, why_for_run, why_plain_lines,
};
use deadreckon_protocol::{RunEvent, SpendRecord, TraceRecord};

#[derive(Debug)]
pub(crate) struct AttachCommandArgs {
    pub(crate) run_id: String,
    pub(crate) no_hints: bool,
    pub(crate) plain: bool,
    pub(crate) json: bool,
    pub(crate) why: bool,
    pub(crate) view: AttachViewMode,
    pub(crate) visual: NarrativeVisualMode,
    pub(crate) narrative_provider: Option<String>,
    pub(crate) narrative_max_spend: Option<f64>,
}

pub(crate) async fn attach_command(args: AttachCommandArgs) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let narrative_config = NarrativeAttachConfig {
        provider: args.narrative_provider.clone(),
        max_spend_usd: args.narrative_max_spend,
    };
    let mut parent_plan = None;
    let run_ref = args.run_id.clone();
    // Shakedown P6: one resolution replaces the hand-rolled
    // campaign/plan-child/run/plan/chain cascade. Every arm below is the body
    // that was already here, so the characterization goldens should not move.
    let resolved = super::reference::resolve_ref(
        &paths,
        super::reference::RefQuery {
            reference: Some(&run_ref),
            accepts: super::reference::RefKinds::ALL,
            all_scopes: false,
            verb: "attach",
        },
    )?;
    let state = match resolved {
        super::reference::ResolvedRef::Campaign {
            dir: campaign_dir,
            campaign,
        } => {
            let state =
                commands::campaign::CampaignAttachState::new(&paths, &campaign_dir, *campaign);
            if args.why {
                print!(
                    "{}",
                    commands::campaign::campaign_why_failed_report(
                        &paths,
                        &state.campaign,
                        state.rollup.as_ref()
                    )
                );
            } else if args.view.is_narrative() && args.json {
                print_campaign_narrative_json(&paths, &state.campaign, args.visual)?;
            } else if args.view.is_narrative() && (!io::stdout().is_terminal() || args.plain) {
                print_campaign_narrative_plain(&paths, &state.campaign, args.visual)?;
            } else if args.json {
                print!(
                    "{}",
                    commands::campaign::campaign_attach_json_text(Some(&paths), &state)?
                );
            } else if io::stdout().is_terminal() && !args.plain {
                let show_hints = completion_hints_enabled(args.no_hints);
                print_attach_banner("campaign", &state.campaign.campaign_id);
                attach_campaign_tui(
                    &paths,
                    state,
                    show_hints,
                    args.view,
                    args.visual,
                    narrative_config.clone(),
                )
                .await?;
            } else {
                print!(
                    "{}",
                    commands::campaign::campaign_attach_summary(
                        Some(&paths),
                        &state.campaign,
                        state.rollup.as_ref(),
                    )
                );
            }
            return Ok(());
        }
        super::reference::ResolvedRef::Run(state) => *state,
        super::reference::ResolvedRef::PlanChild { selection, state } => {
            parent_plan = Some(AttachParentPlan {
                plan_id: selection.plan_id,
                task_id: selection.task_id,
                campaign_parent: None,
            });
            *state
        }
        super::reference::ResolvedRef::Plan(plan) => {
            let plan = *plan;
            let show_hints = completion_hints_enabled(args.no_hints);
            if args.view.is_narrative() && args.json {
                print_plan_narrative_json(&paths, &plan, args.visual)?;
            } else if args.view.is_narrative() && (!io::stdout().is_terminal() || args.plain) {
                print_plan_narrative_plain(&paths, &plan, args.visual)?;
            } else if io::stdout().is_terminal() && !args.plain && !args.json {
                print_attach_banner("plan", &plan.plan_id);
                attach_plan_session(
                    &paths,
                    &plan.plan_id,
                    show_hints,
                    args.view,
                    args.visual,
                    &narrative_config,
                    None,
                )
                .await?;
            } else {
                print_plan_summary(&paths, &plan, show_hints);
            }
            return Ok(());
        }
        super::reference::ResolvedRef::Chain(chain) => {
            let chain_id = chain.chain_id.clone();
            if args.why {
                return commands::chain::chain_show_command(&paths, &chain_id, true, false);
            }
            if args.view.is_narrative() {
                let chain = *chain;
                let events =
                    read_jsonl::<ChainEvent>(&paths.chain_events(&chain_id)).unwrap_or_default();
                if args.json {
                    print!("{}", chain_narrative_json_text(&chain, &events)?);
                } else if io::stdout().is_terminal() && !args.plain {
                    return commands::chain::chain_attach_command_with_view(
                        &paths, &chain_id, args.plain, true,
                    )
                    .await;
                } else {
                    print!("{}", chain_narrative_plain_text(&chain, &events));
                }
                return Ok(());
            }
            return commands::chain::chain_attach_command(&paths, &chain_id, args.plain).await;
        }
    };
    let run_id = state.run_id.clone();
    let show_hints = completion_hints_enabled(args.no_hints);
    if args.why {
        print!("{}", run_why_plain_text(&state)?);
        return Ok(());
    }
    if args.view.is_narrative() && args.json {
        print_run_narrative_json(&state, parent_plan.as_ref(), args.visual)?;
        return Ok(());
    }
    if args.view.is_narrative() && (!io::stdout().is_terminal() || args.plain) {
        print_run_narrative_plain(&state, parent_plan.as_ref(), args.visual)?;
        return Ok(());
    }
    if io::stdout().is_terminal() && !args.plain && !args.json {
        print_attach_banner("run", &run_id);
        let mut final_run_id = run_id.clone();
        if let Some(mut parent) = parent_plan {
            // The run <-> plan round trip: exit keys on a parented run pop to
            // the plan surface; Enter on a zoomed run node there promotes back
            // into a full run surface, until the operator detaches from the plan.
            let mut current_run = run_id.clone();
            loop {
                let outcome = attach_tui_with_parent(
                    &paths,
                    &current_run,
                    show_hints,
                    Some(parent.clone()),
                    args.view,
                    args.visual,
                    narrative_config.clone(),
                )
                .await?;
                final_run_id = current_run.clone();
                match outcome {
                    RunAttachOutcome::Detach => break,
                    RunAttachOutcome::BackToPlan => {
                        print_attach_banner("plan", &parent.plan_id);
                        match attach_plan_tui(
                            &paths,
                            &parent.plan_id.clone(),
                            show_hints,
                            args.view,
                            args.visual,
                            narrative_config.clone(),
                            parent.campaign_parent.clone(),
                        )
                        .await?
                        {
                            PlanAttachOutcome::Detach => break,
                            PlanAttachOutcome::PromoteRun { run_id, task_id } => {
                                parent.task_id = task_id;
                                print_attach_banner("run", &run_id);
                                current_run = run_id;
                            }
                        }
                    }
                }
            }
        } else {
            attach_tui(
                &paths,
                &run_id,
                show_hints,
                args.view,
                args.visual,
                narrative_config.clone(),
            )
            .await?;
        }
        let state = load_run(&paths, &final_run_id)?;
        if state.status == RunStatus::Completed && show_hints {
            print_exit_summary_card(&state, &RunLoopOutcome::Done, args.plain, true);
            print_chain_context_for_working(&state.working_dir);
        }
        return Ok(());
    }
    if let Some(parent_plan) = parent_plan.as_ref() {
        println!(
            "plan {} / {} -> run {}",
            ui_id(run_prefix(&parent_plan.plan_id)),
            ui_id(&parent_plan.task_id),
            ui_id(run_prefix(&state.run_id))
        );
    }
    if state.status == RunStatus::Completed && show_hints {
        print_exit_summary_card(&state, &RunLoopOutcome::Done, args.plain, true);
        print_chain_context_for_working(&state.working_dir);
    } else {
        print_run_summary(&state);
    }
    print_steer_inbox_state(&state)?;
    Ok(())
}

fn print_steer_inbox_state(state: &deadreckon_core::PipelineState) -> Result<()> {
    for entry in deadreckon_core::steer_inbox::read_steer_inbox(&state.run_root)? {
        let text = one_line(&entry.text, 120);
        match entry.status {
            deadreckon_core::steer_inbox::SteerStatus::Pending => {
                println!("steer pending: {text}");
            }
            deadreckon_core::steer_inbox::SteerStatus::Delivered => {
                let turn_id = entry.delivered_turn_id.as_deref().unwrap_or("unknown");
                println!("steer delivered (turn {turn_id}): {text}");
            }
        }
    }
    Ok(())
}

pub(crate) fn dispatch_run_command_mode(
    state: &deadreckon_core::PipelineState,
    action: RunCommandModeAction,
) -> Result<String> {
    match action {
        RunCommandModeAction::Steer(text) => {
            commands::steer::queue_steer_for_state(
                state,
                deadreckon_core::steer_inbox::SteerSource::Tui,
                text,
            )?;
            Ok("delivery will begin on the active or next Codex turn".to_string())
        }
    }
}

pub(crate) fn run_why_plain_text(state: &deadreckon_core::PipelineState) -> Result<String> {
    let report = why_for_run(state)?;
    let mut output = why_plain_lines(&report).join("\n");
    output.push('\n');
    Ok(output)
}

fn print_run_narrative_plain(
    state: &deadreckon_core::PipelineState,
    parent_plan: Option<&AttachParentPlan>,
    visual: NarrativeVisualMode,
) -> Result<()> {
    print!("{}", run_narrative_plain_text(state, parent_plan, visual)?);
    Ok(())
}

pub(crate) fn run_narrative_plain_text(
    state: &deadreckon_core::PipelineState,
    parent_plan: Option<&AttachParentPlan>,
    visual: NarrativeVisualMode,
) -> Result<String> {
    let projection = run_projection_for_plain(state, parent_plan, visual)?;
    let mut output = narrative::narrative_plain_lines(&projection, visual).join("\n");
    output.push('\n');
    Ok(output)
}

fn print_run_narrative_json(
    state: &deadreckon_core::PipelineState,
    parent_plan: Option<&AttachParentPlan>,
    visual: NarrativeVisualMode,
) -> Result<()> {
    println!("{}", run_narrative_json_text(state, parent_plan, visual)?);
    Ok(())
}

pub(crate) fn run_narrative_json_text(
    state: &deadreckon_core::PipelineState,
    parent_plan: Option<&AttachParentPlan>,
    visual: NarrativeVisualMode,
) -> Result<String> {
    let projection = run_projection_for_plain(state, parent_plan, visual)?;
    Ok(serde_json::to_string_pretty(&projection)?)
}

fn print_plan_narrative_plain(
    paths: &DeadreckonPaths,
    plan: &Plan,
    visual: NarrativeVisualMode,
) -> Result<()> {
    print!("{}", plan_narrative_plain_text(paths, plan, visual)?);
    Ok(())
}

fn plan_narrative_plain_text(
    paths: &DeadreckonPaths,
    plan: &Plan,
    visual: NarrativeVisualMode,
) -> Result<String> {
    let projection = plan_projection_for_plain(paths, plan)?;
    let mut output = narrative::narrative_plain_lines(&projection, visual).join("\n");
    output.push('\n');
    Ok(output)
}

fn print_plan_narrative_json(
    paths: &DeadreckonPaths,
    plan: &Plan,
    _visual: NarrativeVisualMode,
) -> Result<()> {
    println!("{}", plan_narrative_json_text(paths, plan)?);
    Ok(())
}

fn plan_narrative_json_text(paths: &DeadreckonPaths, plan: &Plan) -> Result<String> {
    let projection = plan_projection_for_plain(paths, plan)?;
    Ok(serde_json::to_string_pretty(&projection)?)
}

fn run_projection_for_plain(
    state: &deadreckon_core::PipelineState,
    parent_plan: Option<&AttachParentPlan>,
    visual: NarrativeVisualMode,
) -> Result<narrative::NarrativeProjection> {
    let run_view = deadreckon_core::RunView::from_state(state).ok();
    let spend = read_jsonl::<SpendRecord>(&state.run_root.join("spend.jsonl"))?;
    let traces = read_jsonl::<TraceRecord>(&state.run_root.join("traces.jsonl"))?;
    let events = read_jsonl::<RunEvent>(&state.run_root.join(RUN_EVENTS_JSONL))?;
    let live = collect_attach_live(state);
    let tui_state = AttachTuiState {
        view: AttachViewMode::Narrative,
        visual,
        parent_plan: parent_plan.cloned(),
        ..AttachTuiState::default()
    };
    run_narrative_projection(state, &spend, &traces, &events, &live, &tui_state).or_else(|_| {
        let input = RunNarrativeRenderInput {
            state,
            run_view: run_view.as_ref(),
            spend: &spend,
            traces: &traces,
            events: &events,
            live: &live,
            tui_state: &tui_state,
        };
        Ok(build_run_narrative_projection(&input))
    })
}

fn plan_projection_for_plain(
    paths: &DeadreckonPaths,
    plan: &Plan,
) -> Result<narrative::NarrativeProjection> {
    let messages = read_plan_messages(paths, &plan.plan_id).unwrap_or_default();
    let plan_events = read_plan_events_lossy(paths, &plan.plan_id);
    narrative::ensure_plan_projection(&narrative::PlanNarrativeInput {
        paths,
        plan,
        messages: &messages,
        plan_events: &plan_events,
        feed_events: &[],
        selected: 0,
    })
}

fn print_campaign_narrative_plain(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
    visual: NarrativeVisualMode,
) -> Result<()> {
    print!(
        "{}",
        campaign_narrative_plain_text(paths, campaign, visual)?
    );
    Ok(())
}

pub(crate) fn campaign_narrative_plain_text(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
    visual: NarrativeVisualMode,
) -> Result<String> {
    let projection = campaign_projection_for_plain(paths, campaign)?;
    let mut output = narrative::narrative_plain_lines(&projection, visual).join("\n");
    output.push('\n');
    Ok(output)
}

fn print_campaign_narrative_json(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
    _visual: NarrativeVisualMode,
) -> Result<()> {
    let projection = campaign_projection_for_plain(paths, campaign)?;
    println!("{}", serde_json::to_string_pretty(&projection)?);
    Ok(())
}

fn campaign_projection_for_plain(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
) -> Result<narrative::NarrativeProjection> {
    narrative::ensure_campaign_projection(&narrative::CampaignNarrativeInput {
        paths,
        campaign,
        selected: 0,
    })
}

async fn attach_campaign_tui(
    paths: &DeadreckonPaths,
    mut state: commands::campaign::CampaignAttachState,
    show_hints: bool,
    initial_view: AttachViewMode,
    initial_visual: NarrativeVisualMode,
    narrative_config: NarrativeAttachConfig,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut feed = commands::campaign::CampaignEventFeed::new(
        paths.clone(),
        state.campaign_dir.clone(),
        state.campaign.campaign_id.clone(),
    );
    let tick_budget = attach_tick_budget_from_config(paths);
    let mut input_events = AttachInputEvents::new(tick_budget);
    let mut pending_input_at: Option<Instant> = None;

    let mut show_help = false;
    let result = loop {
        let mut tick = AttachTickTiming::new(AttachSurface::Campaign, tick_budget);
        let stage_started = Instant::now();
        let events = feed.refresh(Duration::ZERO).await;
        state.apply_feed_events(events);
        tick.record_since(AttachLoopStage::EventFeed, stage_started);

        let stage_started = Instant::now();
        if let Err(error) = state.refresh(paths) {
            state
                .feed
                .push_back(commands::campaign::CampaignFeedEvent::Warning {
                    message: format!("campaign refresh failed: {error}"),
                });
        }
        tick.record_since(AttachLoopStage::LoadState, stage_started);

        let stage_started = Instant::now();
        terminal.draw(|frame| {
            render_campaign_attach(frame, &state);
            if show_help {
                render_help_overlay(frame, AttachHelpMode::Campaign);
            }
        })?;
        tick.record_since(AttachLoopStage::Draw, stage_started);
        if let Some(started_at) = pending_input_at.take() {
            tick.record(AttachLoopStage::InputToFrame, started_at.elapsed());
        }

        let stage_started = Instant::now();
        let input_event = input_events.next_event().await?;
        tick.record_since(AttachLoopStage::InputPoll, stage_started);
        drop(tick.slow_sync_stages());
        drop(tick.slow_stage_labels());
        let _ = tick.frame_exceeded();
        let _ = tick.input_to_frame_exceeded();

        if let Some(Event::Key(key)) = input_event {
            pending_input_at = Some(Instant::now());
            match handle_help_key(show_help, key) {
                HelpKeyAction::Open => {
                    show_help = true;
                    continue;
                }
                HelpKeyAction::Close => {
                    show_help = false;
                    continue;
                }
                HelpKeyAction::NotHandled => {}
            }
            match handle_campaign_key(&mut state, key) {
                CampaignAttachKeyAction::None | CampaignAttachKeyAction::Refresh => {}
                CampaignAttachKeyAction::Back | CampaignAttachKeyAction::Quit => break Ok(()),
                CampaignAttachKeyAction::DrillInto { sub_id, plan_id } => {
                    if load_plan(paths, &plan_id).is_err() {
                        state
                            .feed
                            .push_back(commands::campaign::CampaignFeedEvent::Warning {
                                message: format!("sub-plan {sub_id} unavailable: {plan_id}"),
                            });
                        continue;
                    }
                    suspend_tui(&mut terminal)?;
                    let parent_campaign = AttachCampaignParent {
                        campaign_id: state.campaign.campaign_id.clone(),
                        sub_id,
                    };
                    let child_result = attach_plan_session(
                        paths,
                        &plan_id,
                        show_hints,
                        initial_view,
                        initial_visual,
                        &narrative_config,
                        Some(parent_campaign),
                    )
                    .await;
                    if let Err(err) = &child_result {
                        print_error(err);
                        let _ =
                            wait_for_return("press Enter/q/Esc to return to campaign attach...");
                    }
                    resume_tui(&mut terminal)?;
                }
            }
        }
    };

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    result
}

/// Drive one plan attach session, following promote/back navigation between
/// the plan surface and full child-run surfaces until the operator detaches:
/// Enter on a zoomed run node opens the full run frame; b/Backspace/q/Esc
/// there pops back to the plan surface (the footer's "back to plan" promise).
async fn attach_plan_session(
    paths: &DeadreckonPaths,
    plan_id: &str,
    show_hints: bool,
    view: AttachViewMode,
    visual: NarrativeVisualMode,
    narrative_config: &NarrativeAttachConfig,
    parent_campaign: Option<AttachCampaignParent>,
) -> Result<()> {
    loop {
        match attach_plan_tui(
            paths,
            plan_id,
            show_hints,
            view,
            visual,
            narrative_config.clone(),
            parent_campaign.clone(),
        )
        .await?
        {
            PlanAttachOutcome::Detach => return Ok(()),
            PlanAttachOutcome::PromoteRun { run_id, task_id } => {
                let parent = AttachParentPlan {
                    plan_id: plan_id.to_string(),
                    task_id,
                    campaign_parent: parent_campaign.clone(),
                };
                print_attach_banner("run", &run_id);
                match attach_tui_with_parent(
                    paths,
                    &run_id,
                    show_hints,
                    Some(parent),
                    view,
                    visual,
                    narrative_config.clone(),
                )
                .await?
                {
                    RunAttachOutcome::Detach => return Ok(()),
                    RunAttachOutcome::BackToPlan => {
                        print_attach_banner("plan", plan_id);
                    }
                }
            }
        }
    }
}

async fn attach_plan_tui(
    paths: &DeadreckonPaths,
    plan_id: &str,
    show_hints: bool,
    initial_view: AttachViewMode,
    initial_visual: NarrativeVisualMode,
    narrative_config: NarrativeAttachConfig,
    parent_campaign: Option<AttachCampaignParent>,
) -> Result<PlanAttachOutcome> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut selected = 0_usize;
    let mut plan = load_plan(paths, plan_id)?;
    let mut messages: Vec<PlanMessage>;
    let mut plan_events = Vec::<PlanEvent>::new();
    let mut feed_events = Vec::<PlanFeedEvent>::new();
    let mut feed = PlanEventBus::file_tail(paths.clone(), plan_id.to_string());
    let mut view = initial_view;
    let mut visual = initial_visual;
    let mut narrative_scroll = 0_usize;
    let mut narrative_notice = None;
    let mut quiet_tracker = NarrativeQuietRefreshTracker::new(Utc::now());
    let mut narrative_refresh_job: Option<AttachPlanNarrativeRefreshJob> = None;
    let mut narrative_projection_cache = AttachNarrativeProjectionCache::default();
    let mut show_help = false;
    let mut zoom = VoyageZoomState::default();
    let tick_budget = attach_tick_budget_from_config(paths);
    let mut input_events = AttachInputEvents::new(tick_budget);
    let mut pending_input_at: Option<Instant> = None;

    let result = loop {
        let mut tick = AttachTickTiming::new(AttachSurface::Plan, tick_budget);
        let now = Utc::now();
        let stage_started = Instant::now();
        let new_feed_events = feed.refresh(Duration::ZERO).await;
        tick.record_since(AttachLoopStage::EventFeed, stage_started);
        let event_refresh = plan_narrative_refresh_trigger(&new_feed_events);
        quiet_tracker.observe_event_trigger(event_refresh, now);
        for event in new_feed_events {
            match event {
                PlanFeedEvent::Plan { event } => {
                    plan_events.push(event.clone());
                    feed_events.push(PlanFeedEvent::Plan { event });
                }
                PlanFeedEvent::Snapshot { plan: snapshot } => {
                    plan = (*snapshot).clone();
                    feed_events.push(PlanFeedEvent::Snapshot { plan: snapshot });
                }
                other => feed_events.push(other),
            }
        }
        if plan_events.len() > 1_000 {
            let drain = plan_events.len().saturating_sub(1_000);
            plan_events.drain(0..drain);
        }
        if feed_events.len() > 1_000 {
            let drain = feed_events.len().saturating_sub(1_000);
            feed_events.drain(0..drain);
        }
        let stage_started = Instant::now();
        messages = read_plan_messages(paths, plan_id).unwrap_or_default();
        tick.record_since(AttachLoopStage::PlanMessages, stage_started);
        if let Some(notice) = poll_plan_narrative_refresh_job(&mut narrative_refresh_job).await {
            narrative_projection_cache.invalidate();
            narrative_notice = Some(notice);
        }
        if selected >= plan.tasks.len() {
            selected = plan.tasks.len().saturating_sub(1);
        }
        let auto_refresh = event_refresh.or_else(|| {
            if view.is_narrative() {
                quiet_tracker.maybe_trigger(
                    plan.status == PlanStatus::Forked,
                    narrative::NarrativeCadence::default().quiet_seconds,
                    now,
                )
            } else {
                None
            }
        });
        if let Some(kind) = auto_refresh
            && view.is_narrative()
        {
            let stage_started = Instant::now();
            let refresh_input = PlanNarrativeRefreshInput {
                paths,
                plan: &plan,
                messages: &messages,
                plan_events: &plan_events,
                feed_events: &feed_events,
                selected,
                config: &narrative_config,
            };
            narrative_notice = Some(start_or_coalesce_plan_narrative_refresh_job(
                &mut narrative_refresh_job,
                plan_narrative_refresh_request(&refresh_input, kind),
                now,
            ));
            tick.record_since(AttachLoopStage::ProviderNarrativeRefresh, stage_started);
        }
        let narrative_projection = if view.is_narrative() {
            let input = narrative::PlanNarrativeInput {
                paths,
                plan: &plan,
                messages: &messages,
                plan_events: &plan_events,
                feed_events: &feed_events,
                selected,
            };
            narrative_projection_cache
                .refresh_plan(&input)
                .or_else(|_| Ok::<_, CliError>(narrative::build_plan_projection(&input)))
                .ok()
        } else {
            None
        };
        let stage_started = Instant::now();
        let selected_node = plan
            .tasks
            .get(selected)
            .map(|task| NodeId::task(&plan.plan_id, &task.task_id));
        terminal.draw(|frame| {
            render_plan_attach(
                frame,
                paths,
                &plan,
                &PlanAttachRenderState {
                    messages: &messages,
                    plan_events: &plan_events,
                    feed_events: &feed_events,
                    selected,
                    selected_node: selected_node.as_ref(),
                    zoomed_node: zoom.zoomed(),
                    show_hints,
                    view,
                    visual,
                    campaign_parent: parent_campaign.as_ref(),
                    narrative_notice: narrative_notice.as_deref(),
                    narrative_projection: narrative_projection.as_ref(),
                    narrative_scroll,
                },
            );
            if show_help {
                render_help_overlay(frame, AttachHelpMode::Plan);
            }
        })?;
        tick.record_since(AttachLoopStage::Draw, stage_started);
        if let Some(started_at) = pending_input_at.take() {
            tick.record(AttachLoopStage::InputToFrame, started_at.elapsed());
        }

        let stage_started = Instant::now();
        let input_event = input_events.next_event().await?;
        tick.record_since(AttachLoopStage::InputPoll, stage_started);
        drop(tick.slow_sync_stages());
        drop(tick.slow_stage_labels());
        let _ = tick.frame_exceeded();
        let _ = tick.input_to_frame_exceeded();
        if let Some(event) = input_event {
            if let Event::Key(key) = event {
                pending_input_at = Some(Instant::now());
                match handle_help_key(show_help, key) {
                    HelpKeyAction::Open => {
                        show_help = true;
                        continue;
                    }
                    HelpKeyAction::Close => {
                        show_help = false;
                        continue;
                    }
                    HelpKeyAction::NotHandled => {}
                }
            }
            match event {
                Event::Key(key)
                    if key.code == KeyCode::Esc
                        && key.modifiers.is_empty()
                        && zoom.zoomed().is_some() =>
                {
                    let _ = zoom.escape();
                }
                Event::Key(key) if attach_should_quit(key) => {
                    break Ok(PlanAttachOutcome::Detach);
                }
                Event::Key(key) if key.code == KeyCode::Char('n') && key.modifiers.is_empty() => {
                    view = toggle_attach_view(view);
                    narrative_scroll = 0;
                }
                Event::Key(key) if key.code == KeyCode::Char('v') && key.modifiers.is_empty() => {
                    visual = visual.next();
                }
                Event::Key(key) if key.code == KeyCode::Char('r') && key.modifiers.is_empty() => {
                    view = AttachViewMode::Narrative;
                    let stage_started = Instant::now();
                    let refresh_input = PlanNarrativeRefreshInput {
                        paths,
                        plan: &plan,
                        messages: &messages,
                        plan_events: &plan_events,
                        feed_events: &feed_events,
                        selected,
                        config: &narrative_config,
                    };
                    narrative_notice = Some(start_or_coalesce_plan_narrative_refresh_job(
                        &mut narrative_refresh_job,
                        plan_narrative_refresh_request(
                            &refresh_input,
                            NarrativeRefreshKind::Manual,
                        ),
                        Utc::now(),
                    ));
                    tick.record_since(AttachLoopStage::ProviderNarrativeRefresh, stage_started);
                    // No redundant immediate redraw: the loop redraws at the top of the
                    // next iteration (which begins right after input handling), and the
                    // background refresh job invalidates the narrative cache on completion.
                }
                Event::Key(key) if key.code == KeyCode::Enter => {
                    let task = plan.tasks.get(selected);
                    let zoom_target = task
                        .map(|task| {
                            task.child_run_id
                                .as_deref()
                                .map(NodeId::run)
                                .unwrap_or_else(|| NodeId::task(&plan.plan_id, &task.task_id))
                        })
                        .unwrap_or_else(|| NodeId::plan(&plan.plan_id));
                    if let Some(run_id) = crate::tui::panes::voyage::zoomed_run_to_promote(
                        &zoom,
                        &zoom_target,
                        task.and_then(|task| task.child_run_id.as_deref()),
                    ) {
                        let task_id = task.map(|task| task.task_id.clone()).unwrap_or_default();
                        break Ok(PlanAttachOutcome::PromoteRun { run_id, task_id });
                    }
                    let _ = zoom.enter(zoom_target);
                }
                Event::Key(key) if view.is_narrative() => {
                    let total = narrative_projection.as_ref().map_or(0, |projection| {
                        let mut lines = narrative::narrative_plain_lines(projection, visual).len();
                        if narrative_notice.is_some() {
                            lines += 1;
                        }
                        lines
                    });
                    let rows =
                        usize::from(crate::tui::PLAN_NARRATIVE_AREA_HEIGHT.saturating_sub(2));
                    let mut nav = NarrativeScrollNav {
                        scroll: &mut narrative_scroll,
                        max: total.saturating_sub(rows),
                    };
                    crate::tui::navigation::dispatch_navigation(&mut nav, key);
                }
                Event::Key(key) => {
                    let mut nav = PlanNav {
                        selected: &mut selected,
                        count: plan.tasks.len(),
                    };
                    crate::tui::navigation::dispatch_navigation(&mut nav, key);
                }
                _ => {}
            }
        }
    };

    cancel_plan_narrative_refresh_job(&mut narrative_refresh_job);
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    result
}

async fn attach_tui(
    paths: &DeadreckonPaths,
    run_id: &str,
    show_completion_actions: bool,
    initial_view: AttachViewMode,
    initial_visual: NarrativeVisualMode,
    narrative_config: NarrativeAttachConfig,
) -> Result<()> {
    attach_tui_with_parent(
        paths,
        run_id,
        show_completion_actions,
        None,
        initial_view,
        initial_visual,
        narrative_config,
    )
    .await
    .map(|_| ())
}

async fn attach_tui_with_parent(
    paths: &DeadreckonPaths,
    run_id: &str,
    show_completion_actions: bool,
    parent_plan: Option<AttachParentPlan>,
    initial_view: AttachViewMode,
    initial_visual: NarrativeVisualMode,
    narrative_config: NarrativeAttachConfig,
) -> Result<RunAttachOutcome> {
    let initial_state = load_run(paths, run_id)?;
    let mut event_feed =
        tui_events::TuiEventFeed::file_tail(initial_state.run_root.join(RUN_EVENTS_JSONL));
    let mut events = event_feed.refresh(std::time::Duration::ZERO).await?;
    let tick_budget = attach_tick_budget_from_config(paths);
    let mut input_events = AttachInputEvents::new(tick_budget);
    let mut pending_input_at: Option<Instant> = None;
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let motion_policy = config_defaults(paths)
        .map(|defaults| defaults.motion_policy(true, false))
        .unwrap_or_else(|_| MotionPolicy::default_for_environment(true, false));
    let mut tui_state = AttachTuiState {
        show_completion_actions,
        parent_plan,
        view: initial_view,
        visual: initial_visual,
        motion_policy,
        ..AttachTuiState::default()
    };
    let mut quiet_tracker = NarrativeQuietRefreshTracker::new(Utc::now());
    let mut acceptance_tracker = NarrativeAcceptanceRefreshTracker::default();
    let mut narrative_refresh_job: Option<AttachRunNarrativeRefreshJob> = None;
    let mut spend_tail =
        AttachJsonlTail::<SpendRecord>::new(initial_state.run_root.join("spend.jsonl"));
    let mut trace_tail =
        AttachJsonlTail::<TraceRecord>::new(initial_state.run_root.join("traces.jsonl"));
    let mut provider_activity_cache = AttachProviderActivityCache::new(&initial_state);
    let mut narrative_projection_cache = AttachNarrativeProjectionCache::default();
    let mut show_help = false;

    let result = loop {
        let mut tick = AttachTickTiming::new(AttachSurface::Run, tick_budget);
        let now = Utc::now();
        let stage_started = Instant::now();
        let state = load_run(paths, run_id)?;
        tick.record_since(AttachLoopStage::LoadState, stage_started);
        let stage_started = Instant::now();
        spend_tail.reset_to_path(state.run_root.join("spend.jsonl"));
        trace_tail.reset_to_path(state.run_root.join("traces.jsonl"));
        let spend = spend_tail.refresh()?;
        let traces = trace_tail.refresh()?;
        tick.record_since(AttachLoopStage::ReadJsonl, stage_started);
        let stage_started = Instant::now();
        let new_events = event_feed.refresh(std::time::Duration::ZERO).await?;
        tick.record_since(AttachLoopStage::EventFeed, stage_started);
        let run_event_refresh = run_narrative_refresh_trigger(&new_events);
        events.extend(new_events);
        // Bound the in-memory activity history like the plan loop does (attach.rs:462):
        // the Activity panel only shows a window, and an unbounded Vec makes every
        // frame's line-build O(run length).
        if events.len() > 1_000 {
            let drain = events.len().saturating_sub(1_000);
            events.drain(0..drain);
        }
        let stage_started = Instant::now();
        let provider_activity = provider_activity_cache.refresh(&state);
        let live = collect_attach_live_with_provider_activity(&state, provider_activity);
        tick.record_since(AttachLoopStage::LiveCollect, stage_started);
        let event_refresh =
            run_event_refresh.or_else(|| acceptance_tracker.observe(&live.acceptance));
        quiet_tracker.observe_event_trigger(event_refresh, now);
        if let Some(notice) = poll_run_narrative_refresh_job(&mut narrative_refresh_job).await {
            narrative_projection_cache.invalidate();
            tui_state.record_narrative_refresh(notice);
        }
        let run_view = if tui_state.view.is_narrative() {
            deadreckon_core::RunView::from_state(&state).ok()
        } else {
            None
        };
        tui_state.narrative_projection = if tui_state.view.is_narrative() {
            let input = RunNarrativeRenderInput {
                state: &state,
                run_view: run_view.as_ref(),
                spend,
                traces,
                events: &events,
                live: &live,
                tui_state: &tui_state,
            };
            narrative_projection_cache
                .refresh_run(&input)
                .or_else(|_| Ok::<_, CliError>(build_run_narrative_projection(&input)))
                .ok()
        } else {
            None
        };
        let terminal_size = terminal.size()?;
        let terminal_area =
            ratatui::layout::Rect::new(0, 0, terminal_size.width, terminal_size.height);
        let panel_layout = attach_panel_layout(terminal_area);
        let panel_counts = attach_panel_counts(&state, spend, traces, &events, &live, &tui_state);
        tui_state.clamp(panel_counts, panel_layout.rows);
        tui_state.refresh_effects_for_run(&state, &live, pending_input_at.is_some());
        let auto_refresh = event_refresh.or_else(|| {
            if tui_state.view.is_narrative() {
                quiet_tracker.maybe_trigger(
                    state.status == RunStatus::Executing,
                    narrative::NarrativeCadence::default().quiet_seconds,
                    now,
                )
            } else {
                None
            }
        });
        if let Some(kind) = auto_refresh
            && tui_state.view.is_narrative()
        {
            let stage_started = Instant::now();
            let input = RunNarrativeRenderInput {
                state: &state,
                run_view: run_view.as_ref(),
                spend,
                traces,
                events: &events,
                live: &live,
                tui_state: &tui_state,
            };
            let notice = start_or_coalesce_run_narrative_refresh_job(
                &mut narrative_refresh_job,
                run_narrative_refresh_request(paths, &input, &narrative_config, kind),
                now,
            );
            tui_state.record_narrative_refresh(notice);
            tick.record_since(AttachLoopStage::ProviderNarrativeRefresh, stage_started);
        }
        let stage_started = Instant::now();
        terminal.draw(|frame| {
            render_attach(frame, &state, spend, traces, &events, &live, &tui_state);
            if show_help {
                render_help_overlay(frame, AttachHelpMode::Run);
            }
        })?;
        tui_state.dismiss_discoverability_hint();
        tui_state.clear_active_effect_frames();
        tick.record_since(AttachLoopStage::Draw, stage_started);
        if let Some(started_at) = pending_input_at.take() {
            tick.record(AttachLoopStage::InputToFrame, started_at.elapsed());
        }

        let stage_started = Instant::now();
        let input_event = input_events.next_event().await?;
        tick.record_since(AttachLoopStage::InputPoll, stage_started);
        drop(tick.slow_sync_stages());
        drop(tick.slow_stage_labels());
        let _ = tick.frame_exceeded();
        let _ = tick.input_to_frame_exceeded();
        if let Some(event) = input_event {
            if let Event::Key(key) = event {
                pending_input_at = Some(Instant::now());
                match handle_help_key(show_help, key) {
                    HelpKeyAction::Open => {
                        show_help = true;
                        continue;
                    }
                    HelpKeyAction::Close => {
                        show_help = false;
                        continue;
                    }
                    HelpKeyAction::NotHandled => {}
                }
            }
            match event {
                Event::Key(key)
                    if tui_state.run_command_modal_is_open()
                        || (key.code == KeyCode::Char(':') && key.modifiers.is_empty()) =>
                {
                    if let Some(action) = tui_state.handle_run_command_key(key) {
                        match dispatch_run_command_mode(&state, action) {
                            Ok(notice) => {
                                tui_state.open_run_command_notice("steer queued", notice);
                            }
                            Err(err) => {
                                tui_state.open_run_command_notice(
                                    "steer failed",
                                    one_line(&err.to_string(), 96),
                                );
                            }
                        }
                    }
                }
                Event::Key(key)
                    if run_attach_exit(tui_state.parent_plan.is_some(), key).is_some() =>
                {
                    break Ok(run_attach_exit(tui_state.parent_plan.is_some(), key)
                        .unwrap_or(RunAttachOutcome::Detach));
                }
                Event::Key(key) if key.code == KeyCode::Char('n') && key.modifiers.is_empty() => {
                    tui_state.toggle_view();
                }
                Event::Key(key) if key.code == KeyCode::Char('w') && key.modifiers.is_empty() => {
                    tui_state.open_why();
                }
                Event::Key(key) if key.code == KeyCode::Char('v') && key.modifiers.is_empty() => {
                    tui_state.cycle_visual();
                }
                Event::Key(key) if key.code == KeyCode::Char('r') && key.modifiers.is_empty() => {
                    let provider_stage_started = Instant::now();
                    let input = RunNarrativeRenderInput {
                        state: &state,
                        run_view: run_view.as_ref(),
                        spend,
                        traces,
                        events: &events,
                        live: &live,
                        tui_state: &tui_state,
                    };
                    let notice = start_or_coalesce_run_narrative_refresh_job(
                        &mut narrative_refresh_job,
                        run_narrative_refresh_request(
                            paths,
                            &input,
                            &narrative_config,
                            NarrativeRefreshKind::Manual,
                        ),
                        Utc::now(),
                    );
                    tui_state.record_narrative_refresh(notice);
                    tick.record_since(
                        AttachLoopStage::ProviderNarrativeRefresh,
                        provider_stage_started,
                    );
                    // No redundant immediate redraw: the loop redraws at the top of the
                    // next iteration; the background refresh job invalidates the cache.
                }
                Event::Key(key)
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.is_empty()
                        && read_chain_step_marker(&state.working_dir)?.is_some() =>
                {
                    if let Some(marker) = read_chain_step_marker(&state.working_dir)? {
                        suspend_tui(&mut terminal)?;
                        let action =
                            commands::chain::chain_attach_command(paths, &marker.chain_id, false)
                                .await;
                        if let Err(err) = &action {
                            print_error(err);
                        }
                        resume_tui(&mut terminal)?;
                    }
                }
                Event::Key(key)
                    if tui_state.show_completion_actions
                        && state.status == RunStatus::Completed =>
                {
                    if key.code == KeyCode::Char('d') && key.modifiers.is_empty() {
                        tui_state.toggle_docs();
                    } else if key.code == KeyCode::Char('w') && key.modifiers.is_empty() {
                        tui_state.open_why();
                    } else {
                        match resolve_completion_key(key, tui_state.pending_confirm) {
                            CompletionKeyOutcome::Confirm(action) => {
                                tui_state.pending_confirm = Some(action);
                            }
                            CompletionKeyOutcome::Cancel => {
                                tui_state.pending_confirm = None;
                            }
                            CompletionKeyOutcome::Execute(action) => {
                                tui_state.pending_confirm = None;
                                if let Some(notice) =
                                    run_completion_action(&mut terminal, paths, &state, action)
                                        .await?
                                {
                                    tui_state.record_post_action(notice);
                                }
                            }
                            CompletionKeyOutcome::Ignored => {
                                tui_state.handle_key(key, panel_counts, panel_layout.rows);
                            }
                        }
                    }
                }
                Event::Key(key) => tui_state.handle_key(key, panel_counts, panel_layout.rows),
                Event::Mouse(mouse) => {
                    if let Some(panel) = panel_layout.panel_at(mouse.column, mouse.row) {
                        tui_state.focused_panel = panel;
                    }
                    match mouse.kind {
                        MouseEventKind::ScrollDown => {
                            tui_state.scroll_focused(3, panel_counts, panel_layout.rows)
                        }
                        MouseEventKind::ScrollUp => {
                            tui_state.scroll_focused(-3, panel_counts, panel_layout.rows)
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    };

    cancel_run_narrative_refresh_job(&mut narrative_refresh_job);
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    result
}

#[derive(Debug, PartialEq, Eq)]
enum CompletionKeyOutcome {
    Ignored,
    Execute(CompletionAction),
    Confirm(CompletionAction),
    Cancel,
}

/// Resolve a completion-overlay keystroke given any pending confirmation.
/// Destructive actions (Apply/Abandon) require a two-step confirm: the action
/// key arms `pending`, then `y` runs it and any other key cancels. Abandon is
/// `x` so that `b` is unambiguously "back" (no longer overloaded with Abandon).
fn resolve_completion_key(
    key: KeyEvent,
    pending: Option<CompletionAction>,
) -> CompletionKeyOutcome {
    if let Some(action) = pending {
        return if key.code == KeyCode::Char('y') {
            CompletionKeyOutcome::Execute(action)
        } else {
            CompletionKeyOutcome::Cancel
        };
    }
    let action = match key.code {
        KeyCode::Char('m') => CompletionAction::Materialize,
        KeyCode::Char('e') => CompletionAction::Extend,
        KeyCode::Char('a') => CompletionAction::Apply,
        KeyCode::Char('x') => CompletionAction::Abandon,
        KeyCode::Char('s') => CompletionAction::Show,
        _ => return CompletionKeyOutcome::Ignored,
    };
    if action.is_destructive() {
        CompletionKeyOutcome::Confirm(action)
    } else {
        CompletionKeyOutcome::Execute(action)
    }
}

async fn run_completion_action(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    action: CompletionAction,
) -> Result<Option<AttachActionNotice>> {
    suspend_tui(terminal)?;
    let action_result = match action {
        CompletionAction::Materialize => prompt_materialize_action(paths, state),
        CompletionAction::Extend => prompt_extend_action(state).await,
        CompletionAction::Apply => super::lifecycle::apply_command(
            state.run_id.clone(),
            "squash".to_string(),
            None,
            false,
            false,
            false,
            None,
            false,
        ),
        CompletionAction::Abandon => {
            super::lifecycle::abandon_command(state.run_id.clone(), false, false)
        }
        CompletionAction::Docs => {
            Box::pin(super::doc::doc_command(super::doc::DocCommandArgs {
                run_id: state.run_id.clone(),
                kind: CliDocKind::Narrative,
                export: None,
                polish: false,
                no_confirm: true,
                force: false,
                doc_skill: None,
                doc_provider: None,
                budget_cap: None,
            }))
            .await
        }
        CompletionAction::Show => show_command(ShowCommandArgs {
            run_id: &state.run_id,
            turn: None,
            diff: false,
            raw: None,
            why_failed: false,
            json_output: false,
            flight: false,
            file: None,
        }),
        CompletionAction::Quit => Ok(()),
    };
    if let Err(err) = &action_result {
        print_error(err);
        print_error_hint(err);
    }
    let _ = wait_for_return("press Enter/q/Esc to return to attach...");
    resume_tui(terminal)?;
    Ok(Some(AttachActionNotice {
        action,
        success: action_result.is_ok(),
    }))
}

pub(crate) fn suspend_tui(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    note_tui_suspended();
    Ok(())
}

pub(crate) fn resume_tui(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    note_tui_resumed();
    Ok(())
}

#[cfg(test)]
pub(crate) fn simulate_campaign_drill_cycle() {
    reset_tui_suspend_depth();
    note_tui_suspended();
    note_tui_suspended();
    note_tui_resumed();
    note_tui_resumed();
}

pub(crate) fn attach_should_quit(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
        || (key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL))
}

pub(crate) fn attach_should_return_to_plan(key: KeyEvent) -> bool {
    attach_should_quit(key)
        || matches!(key.code, KeyCode::Backspace)
        || (key.code == KeyCode::Char('b') && key.modifiers.is_empty())
}

/// How a plan attach session ended: a real detach, or the operator promoting
/// a zoomed child run to the full run surface (the plan <-> run round trip).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanAttachOutcome {
    Detach,
    PromoteRun { run_id: String, task_id: String },
}

/// How a run attach session ended. `BackToPlan` is only produced when the run
/// carries a parent plan — the footer promises "back to plan" there, so every
/// exit key pops to the plan surface instead of detaching to the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunAttachOutcome {
    Detach,
    BackToPlan,
}

pub(crate) fn run_attach_exit(has_parent_plan: bool, key: KeyEvent) -> Option<RunAttachOutcome> {
    if has_parent_plan && attach_should_return_to_plan(key) {
        return Some(RunAttachOutcome::BackToPlan);
    }
    if attach_should_quit(key) {
        return Some(RunAttachOutcome::Detach);
    }
    None
}

/// Keys that dismiss a "press Enter to return" prompt: Enter, q, Esc, or
/// Backspace — uniform with the attach exit/return controls (Enter only used to
/// be the sole accepted key).
fn return_key_dismisses(key: KeyEvent) -> bool {
    matches!(
        key.code,
        KeyCode::Enter | KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('q')
    )
}

/// Show `message` and wait for any return key instead of only Enter, so leaving
/// a nested view is uniform across every attach surface.
fn wait_for_return(message: &str) -> Result<()> {
    print!("{message}");
    io::stdout().flush()?;
    enable_raw_mode()?;
    let outcome = loop {
        match event::read() {
            Ok(Event::Key(key)) if return_key_dismisses(key) => break Ok(()),
            Ok(_) => continue,
            Err(err) => break Err(err.into()),
        }
    };
    let _ = disable_raw_mode();
    println!();
    outcome
}

const PLAN_LIST_PAGE: usize = 10;

/// Drives the plan attach task selection through the shared navigation core
/// ([`crate::tui::navigation`]). Arrows/`jk`/`Tab` move the highlighted child by
/// one; `PgUp`/`PgDn` page; `Home`/`End`/`g`/`G` jump to the first/last child —
/// parity with the run panel.
struct PlanNav<'a> {
    selected: &'a mut usize,
    count: usize,
}

impl crate::tui::navigation::NavigableSurface for PlanNav<'_> {
    fn focus_next(&mut self) {
        self.scroll_lines(1);
    }

    fn focus_previous(&mut self) {
        self.scroll_lines(-1);
    }

    fn scroll_lines(&mut self, delta: isize) {
        let max = self.count.saturating_sub(1) as isize;
        *self.selected = (*self.selected as isize + delta).clamp(0, max) as usize;
    }

    fn scroll_page(&mut self, direction: isize) {
        self.scroll_lines(direction.signum() * PLAN_LIST_PAGE as isize);
    }

    fn scroll_to_start(&mut self) {
        *self.selected = 0;
    }

    fn scroll_to_end(&mut self) {
        *self.selected = self.count.saturating_sub(1);
    }

    fn mode_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Left => {
                self.scroll_lines(-1);
                true
            }
            KeyCode::Right => {
                self.scroll_lines(1);
                true
            }
            _ => false,
        }
    }
}

const PLAN_NARRATIVE_PAGE: usize = 5;

/// Scrolls the plan narrative panel through the shared navigation core. The
/// narrative is a single fixed-height window, so `Tab` is a no-op; arrows/`jk`
/// scroll one line, `PgUp`/`PgDn` page, and `Home`/`End`/`g`/`G` jump to the
/// first/last screen — parity with the run narrative panel. `max` is the
/// largest in-bounds offset (`total - visible_rows`), so the window never
/// scrolls past the final line.
struct NarrativeScrollNav<'a> {
    scroll: &'a mut usize,
    max: usize,
}

impl crate::tui::navigation::NavigableSurface for NarrativeScrollNav<'_> {
    fn scroll_lines(&mut self, delta: isize) {
        let next = if delta.is_negative() {
            self.scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.scroll.saturating_add(delta as usize)
        };
        *self.scroll = next.min(self.max);
    }

    fn scroll_page(&mut self, direction: isize) {
        self.scroll_lines(direction.signum() * PLAN_NARRATIVE_PAGE as isize);
    }

    fn scroll_to_start(&mut self) {
        *self.scroll = 0;
    }

    fn scroll_to_end(&mut self) {
        *self.scroll = self.max;
    }
}

#[cfg(test)]
mod nav_tests {
    use super::{
        CompletionKeyOutcome, NarrativeScrollNav, PLAN_LIST_PAGE, PLAN_NARRATIVE_PAGE, PlanNav,
        resolve_completion_key, return_key_dismisses,
    };
    use crate::CompletionAction;
    use crate::tui::navigation::dispatch_navigation;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn plan_nav(selected: &mut usize, count: usize) -> PlanNav<'_> {
        PlanNav { selected, count }
    }

    #[test]
    fn plan_narrative_scroll_clamps_to_window() {
        let mut scroll = 0usize;
        // Down advances one line.
        dispatch_navigation(
            &mut NarrativeScrollNav {
                scroll: &mut scroll,
                max: 7,
            },
            key(KeyCode::Down),
        );
        assert_eq!(scroll, 1, "Down scrolls one line");
        // End jumps to the last in-bounds offset, and further Down is clamped there.
        dispatch_navigation(
            &mut NarrativeScrollNav {
                scroll: &mut scroll,
                max: 7,
            },
            key(KeyCode::End),
        );
        assert_eq!(scroll, 7, "End jumps to the window bottom");
        dispatch_navigation(
            &mut NarrativeScrollNav {
                scroll: &mut scroll,
                max: 7,
            },
            key(KeyCode::Down),
        );
        assert_eq!(scroll, 7, "Down past the bottom stays clamped");
        // PageUp pages back by one visible window.
        dispatch_navigation(
            &mut NarrativeScrollNav {
                scroll: &mut scroll,
                max: 7,
            },
            key(KeyCode::PageUp),
        );
        assert_eq!(
            scroll,
            7 - PLAN_NARRATIVE_PAGE,
            "PgUp pages back one screen"
        );
        // Home returns to the top.
        dispatch_navigation(
            &mut NarrativeScrollNav {
                scroll: &mut scroll,
                max: 7,
            },
            key(KeyCode::Home),
        );
        assert_eq!(scroll, 0, "Home returns to the top");
    }

    #[test]
    fn plan_attach_supports_paging_keys() {
        let count = 30;
        let mut selected = 0;
        dispatch_navigation(&mut plan_nav(&mut selected, count), key(KeyCode::PageDown));
        assert_eq!(selected, PLAN_LIST_PAGE, "PgDn pages forward");
        dispatch_navigation(&mut plan_nav(&mut selected, count), key(KeyCode::PageUp));
        assert_eq!(selected, 0, "PgUp pages back");
        for code in [KeyCode::End, KeyCode::Char('G')] {
            let mut selected = 3;
            dispatch_navigation(&mut plan_nav(&mut selected, count), key(code));
            assert_eq!(selected, count - 1, "{code:?} jumps to last");
        }
        for code in [KeyCode::Home, KeyCode::Char('g')] {
            let mut selected = 7;
            dispatch_navigation(&mut plan_nav(&mut selected, count), key(code));
            assert_eq!(selected, 0, "{code:?} jumps to first");
        }
    }

    #[test]
    fn plan_attach_navigation_matches_run_reference() {
        let count = 10;
        for code in [
            KeyCode::Down,
            KeyCode::Char('j'),
            KeyCode::Tab,
            KeyCode::Right,
        ] {
            let mut selected = 4;
            assert!(dispatch_navigation(
                &mut plan_nav(&mut selected, count),
                key(code)
            ));
            assert_eq!(selected, 5, "{code:?} should advance one");
        }
        for code in [
            KeyCode::Up,
            KeyCode::Char('k'),
            KeyCode::BackTab,
            KeyCode::Left,
        ] {
            let mut selected = 4;
            assert!(dispatch_navigation(
                &mut plan_nav(&mut selected, count),
                key(code)
            ));
            assert_eq!(selected, 3, "{code:?} should retreat one");
        }
        let mut selected = 0;
        dispatch_navigation(&mut plan_nav(&mut selected, count), key(KeyCode::Up));
        assert_eq!(selected, 0, "clamps at top");
        let mut selected = count - 1;
        dispatch_navigation(&mut plan_nav(&mut selected, count), key(KeyCode::Down));
        assert_eq!(selected, count - 1, "clamps at bottom");
    }

    #[test]
    fn apply_requires_confirmation_keystroke() {
        // 'a' only arms the confirm — it must NOT execute immediately.
        assert_eq!(
            resolve_completion_key(key(KeyCode::Char('a')), None),
            CompletionKeyOutcome::Confirm(CompletionAction::Apply)
        );
        // 'y' then runs the armed action.
        assert_eq!(
            resolve_completion_key(key(KeyCode::Char('y')), Some(CompletionAction::Apply)),
            CompletionKeyOutcome::Execute(CompletionAction::Apply)
        );
        // Any other key cancels.
        assert_eq!(
            resolve_completion_key(key(KeyCode::Char('n')), Some(CompletionAction::Apply)),
            CompletionKeyOutcome::Cancel
        );
    }

    #[test]
    fn abandon_requires_confirmation_keystroke() {
        assert_eq!(
            resolve_completion_key(key(KeyCode::Char('x')), None),
            CompletionKeyOutcome::Confirm(CompletionAction::Abandon)
        );
        assert_eq!(
            resolve_completion_key(key(KeyCode::Char('y')), Some(CompletionAction::Abandon)),
            CompletionKeyOutcome::Execute(CompletionAction::Abandon)
        );
        // A single mistyped key cannot fire Abandon.
        assert_eq!(
            resolve_completion_key(key(KeyCode::Esc), Some(CompletionAction::Abandon)),
            CompletionKeyOutcome::Cancel
        );
    }

    #[test]
    fn back_and_abandon_are_distinct_keys() {
        // Abandon is 'x'; 'b' is no longer Abandon (it is reserved for "back").
        assert_eq!(
            resolve_completion_key(key(KeyCode::Char('x')), None),
            CompletionKeyOutcome::Confirm(CompletionAction::Abandon)
        );
        assert_eq!(
            resolve_completion_key(key(KeyCode::Char('b')), None),
            CompletionKeyOutcome::Ignored
        );
        // Non-destructive actions run immediately, no confirm.
        assert_eq!(
            resolve_completion_key(key(KeyCode::Char('s')), None),
            CompletionKeyOutcome::Execute(CompletionAction::Show)
        );
    }

    #[test]
    fn return_prompt_accepts_q_and_esc() {
        for code in [
            KeyCode::Enter,
            KeyCode::Char('q'),
            KeyCode::Esc,
            KeyCode::Backspace,
        ] {
            assert!(
                return_key_dismisses(key(code)),
                "{code:?} should dismiss the return prompt"
            );
        }
        for code in [KeyCode::Char('x'), KeyCode::Down, KeyCode::Char('a')] {
            assert!(
                !return_key_dismisses(key(code)),
                "{code:?} should not dismiss the return prompt"
            );
        }
    }
}
