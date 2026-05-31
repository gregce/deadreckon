use super::super::*;
use crate::tui::{
    AttachActionNotice, AttachParentPlan, AttachTuiState, RunNarrativeRenderInput,
    attach_panel_counts, build_run_narrative_projection, run_narrative_projection,
    toggle_attach_view,
};

#[derive(Debug)]
pub(crate) struct AttachCommandArgs {
    pub(crate) run_id: String,
    pub(crate) no_hints: bool,
    pub(crate) plain: bool,
    pub(crate) json: bool,
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
    if let Some((campaign_dir, campaign)) = commands::campaign::resolve_campaign(&paths, &run_ref)?
    {
        let rollup = deadreckon_core::campaign::read_campaign_rollup(&campaign_dir).ok();
        print!(
            "{}",
            commands::campaign::campaign_attach_summary(Some(&paths), &campaign, rollup.as_ref())
        );
        return Ok(());
    }
    let state = if let Some(selection) = resolve_plan_child_ref(&paths, &run_ref)? {
        parent_plan = Some(AttachParentPlan {
            plan_id: selection.plan_id,
            task_id: selection.task_id,
        });
        load_run(&paths, &selection.run_id)?
    } else {
        match load_cli_run(&paths, &run_ref) {
            Ok(state) => state,
            Err(run_error) => {
                if let Ok(plan_id) = resolve_plan_id(&paths, &run_ref) {
                    let plan = load_plan(&paths, &plan_id)?;
                    let show_hints = completion_hints_enabled(args.no_hints);
                    if args.view.is_narrative() && args.json {
                        print_plan_narrative_json(&paths, &plan, args.visual)?;
                    } else if args.view.is_narrative()
                        && (!io::stdout().is_terminal() || args.plain)
                    {
                        print_plan_narrative_plain(&paths, &plan, args.visual)?;
                    } else if io::stdout().is_terminal() && !args.plain && !args.json {
                        print_attach_banner("plan", &plan.plan_id);
                        attach_plan_tui(
                            &paths,
                            &plan.plan_id,
                            show_hints,
                            args.view,
                            args.visual,
                            narrative_config.clone(),
                        )
                        .await?;
                    } else {
                        print_plan_summary(&paths, &plan, show_hints);
                    }
                    return Ok(());
                }
                if commands::chain::resolve_chain_id(&paths, &run_ref, false).is_ok() {
                    if args.view.is_narrative() || args.json {
                        return print_chain_narrative_refusal(&run_ref, args.json);
                    }
                    return commands::chain::chain_attach_command(&paths, &run_ref, args.plain);
                }
                return Err(run_error);
            }
        }
    };
    let run_id = state.run_id.clone();
    let show_hints = completion_hints_enabled(args.no_hints);
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
        if parent_plan.is_some() {
            attach_tui_with_parent(
                &paths,
                &run_id,
                show_hints,
                parent_plan,
                args.view,
                args.visual,
                narrative_config.clone(),
            )
            .await?;
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
        let state = load_run(&paths, &run_id)?;
        if state.status == RunStatus::Completed && show_hints {
            print_exit_summary_card(&state, &RunLoopOutcome::Done, args.plain);
            print_chain_context_for_working(&state.working_dir);
            print_lifecycle_hints(&state);
        }
        return Ok(());
    }
    if let Some(parent_plan) = parent_plan.as_ref() {
        println!(
            "plan {} / {} -> run {}",
            run_prefix(&parent_plan.plan_id),
            parent_plan.task_id,
            run_prefix(&state.run_id)
        );
    }
    if state.status == RunStatus::Completed && show_hints {
        print_exit_summary_card(&state, &RunLoopOutcome::Done, args.plain);
        print_chain_context_for_working(&state.working_dir);
        print_lifecycle_hints(&state);
    } else {
        print_run_summary(&state);
    }
    Ok(())
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
    run_narrative_projection(state, &spend, &traces, &events, &live, &tui_state)
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

fn print_chain_narrative_refusal(run_ref: &str, json_output: bool) -> Result<()> {
    print!("{}", chain_narrative_refusal_text(run_ref, json_output)?);
    Ok(())
}

pub(crate) fn chain_narrative_refusal_text(run_ref: &str, json_output: bool) -> Result<String> {
    if json_output {
        return Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({
                "status": "unsupported",
                "kind": "chain",
                "id": run_ref,
                "message": "Narrative attach is currently supported for runs, plans, and plan child refs.",
                "try": [
                    "deadreckon chain status",
                    "deadreckon attach <run-id> --view narrative",
                    "deadreckon attach <plan-id> --view narrative"
                ]
            }))?
        ));
    }
    Ok(format!(
        "chain narrative attach is not supported yet\ntry: deadreckon chain status {run_ref}\ntry: deadreckon attach <run-id> --view narrative\ntry: deadreckon attach <plan-id> --view narrative\n"
    ))
}

async fn attach_plan_tui(
    paths: &DeadreckonPaths,
    plan_id: &str,
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
    let mut selected = 0_usize;
    let mut plan = load_plan(paths, plan_id)?;
    let mut messages: Vec<PlanMessage>;
    let mut plan_events = Vec::<PlanEvent>::new();
    let mut feed_events = Vec::<PlanFeedEvent>::new();
    let mut feed = PlanEventBus::file_tail(paths.clone(), plan_id.to_string());
    let mut view = initial_view;
    let mut visual = initial_visual;
    let mut narrative_notice = None;
    let mut quiet_tracker = NarrativeQuietRefreshTracker::new(Utc::now());
    let mut narrative_refresh_job: Option<AttachPlanNarrativeRefreshJob> = None;
    let mut narrative_projection_cache = AttachNarrativeProjectionCache::default();

    let result = loop {
        let mut tick = AttachTickTiming::new(AttachSurface::Plan, AttachTickBudget::default());
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
                    show_hints,
                    view,
                    visual,
                    narrative_notice: narrative_notice.as_deref(),
                    narrative_projection: narrative_projection.as_ref(),
                },
            )
        })?;
        tick.record_since(AttachLoopStage::Draw, stage_started);
        let stage_started = Instant::now();
        let input_ready = event::poll(Duration::from_millis(250))?;
        tick.record_since(AttachLoopStage::InputPoll, stage_started);
        drop(tick.slow_sync_stages());
        drop(tick.slow_stage_labels());
        let _ = tick.frame_exceeded();
        if input_ready {
            match event::read()? {
                Event::Key(key) if attach_should_quit(key) => break Ok(()),
                Event::Key(key) if key.code == KeyCode::Char('n') && key.modifiers.is_empty() => {
                    view = toggle_attach_view(view);
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
                    let stage_started = Instant::now();
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
                                show_hints,
                                view,
                                visual,
                                narrative_notice: narrative_notice.as_deref(),
                                narrative_projection: narrative_projection.as_ref(),
                            },
                        )
                    })?;
                    tick.record_since(AttachLoopStage::Draw, stage_started);
                }
                Event::Key(key)
                    if matches!(
                        key.code,
                        KeyCode::Right | KeyCode::Down | KeyCode::Tab | KeyCode::Char('j')
                    ) =>
                {
                    selected = (selected + 1).min(plan.tasks.len().saturating_sub(1));
                }
                Event::Key(key)
                    if matches!(key.code, KeyCode::Left | KeyCode::Up | KeyCode::Char('k')) =>
                {
                    selected = selected.saturating_sub(1);
                }
                Event::Key(key) if key.code == KeyCode::Enter => {
                    if let Some(run_id) = plan
                        .tasks
                        .get(selected)
                        .and_then(|task| task.child_run_id.as_deref())
                    {
                        if load_run(paths, run_id).is_err() {
                            continue;
                        }
                        let parent_plan = plan.tasks.get(selected).map(|task| AttachParentPlan {
                            plan_id: plan.plan_id.clone(),
                            task_id: task.task_id.clone(),
                        });
                        if cancel_plan_narrative_refresh_job(&mut narrative_refresh_job) {
                            narrative_notice = Some(
                                "plan refresh cancelled while opening child attach".to_string(),
                            );
                        }
                        suspend_tui(&mut terminal)?;
                        let child_result = attach_tui_with_parent(
                            paths,
                            run_id,
                            show_hints,
                            parent_plan,
                            view,
                            visual,
                            narrative_config.clone(),
                        )
                        .await;
                        if let Err(err) = &child_result {
                            print_error(err);
                            let _ = prompt::open("press Enter to return to plan attach...", None);
                        }
                        resume_tui(&mut terminal)?;
                    }
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
}

async fn attach_tui_with_parent(
    paths: &DeadreckonPaths,
    run_id: &str,
    show_completion_actions: bool,
    parent_plan: Option<AttachParentPlan>,
    initial_view: AttachViewMode,
    initial_visual: NarrativeVisualMode,
    narrative_config: NarrativeAttachConfig,
) -> Result<()> {
    let initial_state = load_run(paths, run_id)?;
    let mut event_feed =
        tui_events::TuiEventFeed::file_tail(initial_state.run_root.join(RUN_EVENTS_JSONL));
    let mut events = event_feed.refresh(std::time::Duration::ZERO).await?;
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut tui_state = AttachTuiState {
        show_completion_actions,
        parent_plan,
        view: initial_view,
        visual: initial_visual,
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

    let result = loop {
        let mut tick = AttachTickTiming::new(AttachSurface::Run, AttachTickBudget::default());
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
        tui_state.narrative_projection = if tui_state.view.is_narrative() {
            let input = RunNarrativeRenderInput {
                state: &state,
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
            render_attach(frame, &state, spend, traces, &events, &live, &tui_state)
        })?;
        tick.record_since(AttachLoopStage::Draw, stage_started);

        let stage_started = Instant::now();
        let input_ready = event::poll(Duration::from_millis(200))?;
        tick.record_since(AttachLoopStage::InputPoll, stage_started);
        drop(tick.slow_sync_stages());
        drop(tick.slow_stage_labels());
        let _ = tick.frame_exceeded();
        if input_ready {
            match event::read()? {
                Event::Key(key)
                    if tui_state.parent_plan.is_some() && attach_should_return_to_plan(key) =>
                {
                    break Ok(());
                }
                Event::Key(key) if attach_should_quit(key) => break Ok(()),
                Event::Key(key) if key.code == KeyCode::Char('n') && key.modifiers.is_empty() => {
                    tui_state.toggle_view();
                }
                Event::Key(key) if key.code == KeyCode::Char('v') && key.modifiers.is_empty() => {
                    tui_state.cycle_visual();
                }
                Event::Key(key) if key.code == KeyCode::Char('r') && key.modifiers.is_empty() => {
                    let provider_stage_started = Instant::now();
                    let input = RunNarrativeRenderInput {
                        state: &state,
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
                    let stage_started = Instant::now();
                    terminal.draw(|frame| {
                        render_attach(frame, &state, spend, traces, &events, &live, &tui_state)
                    })?;
                    tick.record_since(AttachLoopStage::Draw, stage_started);
                }
                Event::Key(key)
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.is_empty()
                        && read_chain_step_marker(&state.working_dir)?.is_some() =>
                {
                    if let Some(marker) = read_chain_step_marker(&state.working_dir)? {
                        suspend_tui(&mut terminal)?;
                        let action =
                            commands::chain::chain_attach_command(paths, &marker.chain_id, false);
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
                    } else if let Some(notice) =
                        handle_tui_completion_key(&mut terminal, paths, &state, key).await?
                    {
                        tui_state.record_post_action(notice);
                    } else {
                        tui_state.handle_key(key, panel_counts, panel_layout.rows);
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

async fn handle_tui_completion_key(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    key: KeyEvent,
) -> Result<Option<AttachActionNotice>> {
    let action = match key.code {
        KeyCode::Char('m') => CompletionAction::Materialize,
        KeyCode::Char('e') => CompletionAction::Extend,
        KeyCode::Char('a') => CompletionAction::Apply,
        KeyCode::Char('b') => CompletionAction::Abandon,
        KeyCode::Char('d') => CompletionAction::Docs,
        KeyCode::Char('s') => CompletionAction::Show,
        _ => return Ok(None),
    };

    suspend_tui(terminal)?;
    let action_result = match action {
        CompletionAction::Materialize => prompt_materialize_action(paths, state),
        CompletionAction::Extend => prompt_extend_action(state).await,
        CompletionAction::Apply => apply_command(
            state.run_id.clone(),
            "squash".to_string(),
            None,
            false,
            false,
            false,
            None,
            false,
        ),
        CompletionAction::Abandon => abandon_command(state.run_id.clone(), false, false),
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
        CompletionAction::Show => {
            show_command(&state.run_id, None, false, false, false, false, None)
        }
        CompletionAction::Quit => Ok(()),
    };
    if let Err(err) = &action_result {
        print_error(err);
        print_error_hint(err);
    }
    let _ = prompt::open("press Enter to return to attach...", None);
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
    Ok(())
}

pub(crate) fn resume_tui(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    Ok(())
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
