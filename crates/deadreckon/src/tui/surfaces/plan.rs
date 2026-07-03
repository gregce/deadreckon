use crate::plan_event_bus::PlanFeedEvent;
use crate::tui::attach_state::AttachCampaignParent;
use crate::tui::panes::activity::{scroll_indicator, selection_glyph};
use crate::tui::panes::header::provider_is_metered;
use crate::tui::panes::narrative::{
    NARRATIVE_SPLIT_WIDTH, PLAN_NARRATIVE_AREA_HEIGHT, narrative_list_item, visible_narrative_items,
};
use crate::{
    SpendRecord, TraceRecord, acceptance_status_value, commands, event_line, format_count,
    load_run, narrative, one_line, plan_mode_label, plan_status_label, read_jsonl, read_last_jsonl,
    run_prefix, run_spend_label, task_status_label, ui,
};
use deadreckon_core::{
    DeadreckonPaths, Plan, PlanEvent, PlanEventKind, PlanMessage, PlanMode, PlanTask,
    PlanTaskStatus,
};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

#[derive(Clone, Copy)]
pub(crate) struct PlanAttachRenderState<'a> {
    pub(crate) messages: &'a [PlanMessage],
    pub(crate) plan_events: &'a [PlanEvent],
    pub(crate) feed_events: &'a [PlanFeedEvent],
    pub(crate) selected: usize,
    pub(crate) show_hints: bool,
    pub(crate) view: narrative::AttachViewMode,
    pub(crate) visual: narrative::NarrativeVisualMode,
    pub(crate) campaign_parent: Option<&'a AttachCampaignParent>,
    pub(crate) narrative_notice: Option<&'a str>,
    pub(crate) narrative_projection: Option<&'a narrative::NarrativeProjection>,
    pub(crate) narrative_scroll: usize,
}

pub(crate) fn render_plan_attach(
    frame: &mut ratatui::Frame<'_>,
    paths: &DeadreckonPaths,
    plan: &Plan,
    state: &PlanAttachRenderState<'_>,
) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Min(10),
            Constraint::Length(PLAN_NARRATIVE_AREA_HEIGHT),
            Constraint::Length(2),
        ])
        .split(area);
    let counts = plan_task_counts(plan);
    let campaign_context = state
        .campaign_parent
        .map(|parent| {
            format!(
                "campaign {} / {}  |  ",
                run_prefix(&parent.campaign_id),
                parent.sub_id
            )
        })
        .unwrap_or_default();
    let header = vec![
        Line::from(vec![
            Span::styled(campaign_context, Style::default().fg(Color::Blue)),
            Span::styled("plan ", Style::default().fg(Color::Cyan)),
            Span::styled(
                run_prefix(&plan.plan_id),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "  status {}  mode {:?}  children {}/{}/{}",
                plan_status_label(plan.status),
                plan.mode,
                counts.0,
                counts.1,
                plan.tasks.len()
            )),
        ]),
        Line::from(one_line(
            &plan.root_goal,
            area.width.saturating_sub(4) as usize,
        )),
        Line::from(plan_provider_summary(plan)),
        Line::from(commands::plan::orchestration_parallelism_lines(plan).join("  ")),
        Line::from(format!(
            "capabilities network={:?} deploy={} install={}{}",
            plan.capability_preview.network,
            plan.capability_preview.deploy,
            plan.capability_preview.global_install,
            plan_final_gate_line(paths, plan)
                .map(|line| format!("  final {line}"))
                .unwrap_or_default()
        )),
    ];
    frame.render_widget(
        Paragraph::new(header)
            .block(
                Block::default()
                    .title(format!(
                        "deadreckon plan{}",
                        scroll_indicator(state.selected, 1, plan.tasks.len())
                    ))
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: true }),
        vertical[0],
    );

    let task_area = vertical[1];
    let panes = plan_task_pane_layout(task_area, plan.tasks.len());
    for (index, task) in plan.tasks.iter().enumerate() {
        let Some(rect) = panes.get(index).copied() else {
            continue;
        };
        let is_selected = index == state.selected;
        let title = format!(
            "{} {} {}",
            selection_glyph(is_selected),
            task.task_id,
            task_status_label(task.status)
        );
        let lines =
            plan_task_detail_lines(paths, plan, task, rect.width.saturating_sub(4) as usize)
                .into_iter()
                .enumerate()
                .map(|(line_index, line)| {
                    if line_index == 0 {
                        Line::from(vec![
                            Span::styled(
                                line.split_once("  ")
                                    .map(|(role, _)| role.to_string())
                                    .unwrap_or_else(|| line.clone()),
                                Style::default().fg(Color::Magenta),
                            ),
                            Span::raw(
                                line.split_once("  ")
                                    .map(|(_, rest)| format!("  {rest}"))
                                    .unwrap_or_default(),
                            ),
                        ])
                    } else {
                        Line::from(line)
                    }
                })
                .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(title)
                        .borders(Borders::ALL)
                        .border_style(if is_selected {
                            Style::default().fg(ui::TUI_PALETTE.border_focused)
                        } else {
                            Style::default().fg(ui::TUI_PALETTE.border_idle)
                        }),
                )
                .wrap(Wrap { trim: true }),
            rect,
        );
    }

    if state.view.is_narrative() {
        render_plan_narrative_attach(frame, paths, plan, state, vertical[2]);
    } else {
        let activity = plan_activity_lines(
            state.plan_events,
            state.messages,
            state.feed_events,
            vertical[2].height.saturating_sub(2) as usize,
        );
        let activity_title = if !state.feed_events.is_empty() {
            "plan feed"
        } else if state.plan_events.is_empty() {
            "coordinator messages"
        } else {
            "plan events"
        };
        frame.render_widget(
            List::new(activity).block(Block::default().title(activity_title).borders(Borders::ALL)),
            vertical[2],
        );
    }
    let footer = plan_attach_footer(
        paths,
        plan,
        state.selected,
        state.show_hints,
        state.view,
        state.visual,
    );
    frame.render_widget(Paragraph::new(footer), vertical[3]);
}

pub(crate) fn plan_narrative_title(
    visual: narrative::NarrativeVisualMode,
    scroll: usize,
    rows: usize,
    total: usize,
    split: bool,
) -> String {
    let base = if split {
        "plan narrative".to_string()
    } else {
        format!("plan narrative / {}", visual.label())
    };
    format!("{base}{}", scroll_indicator(scroll, rows, total))
}

fn render_plan_narrative_attach(
    frame: &mut ratatui::Frame<'_>,
    paths: &DeadreckonPaths,
    plan: &Plan,
    state: &PlanAttachRenderState<'_>,
    area: ratatui::layout::Rect,
) {
    let projection = state.narrative_projection.cloned().unwrap_or_else(|| {
        narrative::build_plan_projection(&narrative::PlanNarrativeInput {
            paths,
            plan,
            messages: state.messages,
            plan_events: state.plan_events,
            feed_events: state.feed_events,
            selected: state.selected,
        })
    });
    let mut lines = narrative::narrative_plain_lines(&projection, state.visual);
    if let Some(notice) = state.narrative_notice {
        lines.insert(2, format!("[fresh] {notice}"));
    }
    let rows = area.height.saturating_sub(2) as usize;
    if area.width >= NARRATIVE_SPLIT_WIDTH && state.visual != narrative::NarrativeVisualMode::None {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(area);
        let visual_lines = narrative::graph_ascii_lines(&projection.graph, state.visual);
        lines.retain(|line| !line.starts_with("Visual:"));
        let scroll = state.narrative_scroll.min(lines.len().saturating_sub(rows));
        frame.render_widget(
            List::new(visible_narrative_items(&lines, scroll, rows)).block(
                Block::default()
                    .title(plan_narrative_title(
                        state.visual,
                        scroll,
                        rows,
                        lines.len(),
                        true,
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(ui::TUI_PALETTE.border_focused)),
            ),
            chunks[0],
        );
        frame.render_widget(
            List::new(visual_lines.into_iter().map(narrative_list_item)).block(
                Block::default()
                    .title(format!("visual {}", state.visual.label()))
                    .borders(Borders::ALL),
            ),
            chunks[1],
        );
    } else {
        let scroll = state.narrative_scroll.min(lines.len().saturating_sub(rows));
        frame.render_widget(
            List::new(visible_narrative_items(&lines, scroll, rows)).block(
                Block::default()
                    .title(plan_narrative_title(
                        state.visual,
                        scroll,
                        rows,
                        lines.len(),
                        false,
                    ))
                    .borders(Borders::ALL),
            ),
            area,
        );
    }
}

pub(crate) fn plan_attach_footer(
    paths: &DeadreckonPaths,
    plan: &Plan,
    selected: usize,
    show_hints: bool,
    view: narrative::AttachViewMode,
    visual: narrative::NarrativeVisualMode,
) -> String {
    let mut has_primary_footer_action = false;
    let mut footer = if view.is_narrative() {
        format!(
            "n narrative/activity  v visual={}  r refresh  |  arrows/Tab child  Enter child run  q detach",
            visual.label()
        )
    } else {
        "q/Esc/Ctrl-D detach  |  arrows/Tab focus child  |  Enter child run  |  n narrative  b/Backspace back from child"
            .to_string()
    };
    if let Some(task) = plan.tasks.get(selected) {
        match task.child_run_id.as_deref() {
            None => {
                has_primary_footer_action = true;
                let wait_hint = format!(
                    "Enter waits for child run  |  next deadreckon fork {}",
                    run_prefix(&plan.plan_id)
                );
                if view.is_narrative() {
                    footer = format!("{footer}  |  {wait_hint}");
                } else {
                    footer =
                        format!("q/Esc/Ctrl-D detach  |  arrows/Tab focus child  |  {wait_hint}");
                }
            }
            Some(run_id) if load_run(paths, run_id).is_err() => {
                has_primary_footer_action = true;
                let unavailable_hint = "child detail unavailable  |  next deadreckon list --all";
                if view.is_narrative() {
                    footer = format!("{footer}  |  {unavailable_hint}");
                } else {
                    footer = format!(
                        "q/Esc/Ctrl-D detach  |  arrows/Tab focus child  |  {unavailable_hint}"
                    );
                }
            }
            Some(_) => {}
        }
    }
    if show_hints && !has_primary_footer_action {
        footer.push_str("  |  merge after fork");
    }
    footer.push_str("  |  ? help");
    footer
}

fn plan_task_counts(plan: &Plan) -> (usize, usize) {
    let completed = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Completed)
        .count();
    let running = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Running)
        .count();
    (completed, running)
}

pub(crate) fn plan_provider_summary(plan: &Plan) -> String {
    match plan.mode {
        PlanMode::FullPlan => format!(
            "planner {}  default child {}",
            plan.providers.planner.as_deref().unwrap_or("-"),
            plan.providers.default_child.as_deref().unwrap_or("-")
        ),
        PlanMode::Review => format!(
            "coder {}  reviewer {}",
            plan.providers.coder.as_deref().unwrap_or("-"),
            plan.providers.reviewer.as_deref().unwrap_or("-")
        ),
    }
}

pub(crate) fn plan_repair_label(plan: &Plan, no_repair: bool) -> String {
    if no_repair {
        return "disabled (--no-repair)".to_string();
    }
    let provider = plan
        .providers
        .planner
        .as_deref()
        .or(plan.providers.default_child.as_deref())
        .unwrap_or("config default");
    format!("automatic via {provider}")
}

fn plan_task_pane_layout(
    area: ratatui::layout::Rect,
    task_count: usize,
) -> Vec<ratatui::layout::Rect> {
    if task_count == 0 {
        return Vec::new();
    }
    let rows = if task_count <= 3 { 1 } else { 2 };
    let row_constraints =
        std::iter::repeat_n(Constraint::Ratio(1, rows as u32), rows).collect::<Vec<_>>();
    let row_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(area);
    let mut rects = Vec::new();
    for row in 0..rows {
        let remaining = task_count.saturating_sub(rects.len());
        let columns = remaining.min(if rows == 1 { task_count } else { 3 }).max(1);
        let col_constraints =
            std::iter::repeat_n(Constraint::Ratio(1, columns as u32), columns).collect::<Vec<_>>();
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(col_constraints)
            .split(row_chunks[row]);
        for col in cols.iter().copied().take(remaining) {
            rects.push(col);
            if rects.len() == task_count {
                break;
            }
        }
    }
    rects
}

fn plan_activity_lines(
    plan_events: &[PlanEvent],
    messages: &[PlanMessage],
    feed_events: &[PlanFeedEvent],
    max: usize,
) -> Vec<ListItem<'static>> {
    let mut lines = if !feed_events.is_empty() {
        feed_events
            .iter()
            .rev()
            .take(max.max(1))
            .map(|event| ListItem::new(Line::from(plan_feed_event_line(event))))
            .collect::<Vec<_>>()
    } else if plan_events.is_empty() {
        messages
            .iter()
            .rev()
            .take(max.max(1))
            .map(|message| {
                ListItem::new(Line::from(format!(
                    "{} -> {} {:?}: {}",
                    message.from, message.to, message.kind, message.summary
                )))
            })
            .collect::<Vec<_>>()
    } else {
        plan_events
            .iter()
            .rev()
            .take(max.max(1))
            .map(|event| ListItem::new(Line::from(plan_event_line(event))))
            .collect::<Vec<_>>()
    };
    if lines.is_empty() {
        lines.push(ListItem::new(Line::from("no plan activity yet")));
    }
    lines
}

fn plan_feed_event_line(event: &PlanFeedEvent) -> String {
    match event {
        PlanFeedEvent::Plan { event } => plan_event_line(event),
        PlanFeedEvent::ChildRun {
            task_id,
            run_id,
            event,
        } => format!(
            "{} child {} run {} {}",
            event.timestamp.format("%H:%M:%S"),
            task_id,
            run_prefix(run_id),
            event_line(event, false)
        ),
        PlanFeedEvent::RepairRun { run_id, event } => format!(
            "{} repair run {} {}",
            event.timestamp.format("%H:%M:%S"),
            run_prefix(run_id),
            event_line(event, false)
        ),
        PlanFeedEvent::Snapshot { plan } => format!(
            "{} snapshot status {} children {}",
            chrono::Utc::now().format("%H:%M:%S"),
            plan_status_label(plan.status),
            plan.tasks.len()
        ),
        PlanFeedEvent::Warning { message } => {
            format!(
                "{} warning {message}",
                chrono::Utc::now().format("%H:%M:%S")
            )
        }
    }
}

pub(crate) fn plan_event_line(event: &PlanEvent) -> String {
    format!(
        "{} {}",
        event.timestamp.format("%H:%M:%S"),
        plan_event_summary(&event.event)
    )
}

pub(crate) fn plan_event_summary(event: &PlanEventKind) -> String {
    match event {
        PlanEventKind::PlanCreated { mode, task_count } => {
            format!(
                "plan created mode {} tasks {task_count}",
                plan_mode_label(*mode)
            )
        }
        PlanEventKind::PlanStarted => "plan started".to_string(),
        PlanEventKind::TaskReady { task_id, .. } => format!("{task_id} ready"),
        PlanEventKind::TaskStarted { task_id, .. } => format!("{task_id} started"),
        PlanEventKind::TaskRunDiscovered {
            task_id,
            run_id,
            pid,
            ..
        } => {
            let run = run_id
                .as_deref()
                .map(run_prefix)
                .unwrap_or_else(|| "-".to_string());
            let pid = pid
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| "-".to_string());
            format!("{task_id} run discovered run {run} pid {pid}")
        }
        PlanEventKind::TaskCompleted {
            task_id,
            run_id,
            status,
            ..
        } => {
            let run = run_id
                .as_deref()
                .map(run_prefix)
                .unwrap_or_else(|| "-".to_string());
            format!("{task_id} completed run {run} status {status}")
        }
        PlanEventKind::TaskBlocked {
            task_id, reason, ..
        } => {
            format!("{task_id} blocked: {reason}")
        }
        PlanEventKind::TaskFailed {
            task_id, reason, ..
        } => {
            format!("{task_id} failed: {reason}")
        }
        PlanEventKind::TaskKilled {
            task_id, run_id, ..
        } => {
            let run = run_id
                .as_deref()
                .map(run_prefix)
                .unwrap_or_else(|| "-".to_string());
            format!("{task_id} killed run {run}")
        }
        PlanEventKind::MergeStarted => "merge started".to_string(),
        PlanEventKind::MergeConflict { conflict_count } => {
            format!("merge conflict count {conflict_count}")
        }
        PlanEventKind::MergeRepairPlanned {
            conflict_count,
            provider,
        } => format!(
            "merge repair planned for {conflict_count} conflict(s) with {}",
            provider.as_deref().unwrap_or("no provider")
        ),
        PlanEventKind::MergeRepairStarted { mode } => {
            format!("merge repair started mode {mode}")
        }
        PlanEventKind::MergeRepairRunDiscovered { run_id, pid } => {
            format!(
                "merge repair run {} pid {}",
                run_prefix(run_id),
                pid.map(|pid| pid.to_string())
                    .unwrap_or_else(|| "-".to_string())
            )
        }
        PlanEventKind::MergeRepaired {
            strategy,
            repair_run_id,
        } => format!(
            "merge repaired via {strategy}{}",
            repair_run_id
                .as_deref()
                .map(|run_id| format!(" run {}", run_prefix(run_id)))
                .unwrap_or_default()
        ),
        PlanEventKind::MergeRepairFailed { reason } => {
            format!("merge repair failed: {reason}")
        }
        PlanEventKind::MergeCompleted { merged_run_id } => {
            format!("merge completed run {}", run_prefix(merged_run_id))
        }
        PlanEventKind::PlanCompleted => "plan completed".to_string(),
        PlanEventKind::PlanFailed { reason } => format!("plan failed: {reason}"),
        PlanEventKind::PlanKilled => "plan killed".to_string(),
    }
}

pub(crate) fn plan_final_gate_line(paths: &DeadreckonPaths, plan: &Plan) -> Option<String> {
    let run_id = plan.merged_run_id.as_deref()?;
    let state = load_run(paths, run_id).ok()?;
    Some(format!("gate: {}", acceptance_status_value(&state)))
}

pub(crate) fn plan_task_detail_lines(
    paths: &DeadreckonPaths,
    plan: &Plan,
    task: &PlanTask,
    width: usize,
) -> Vec<String> {
    let mut lines = vec![
        format!(
            "{}  provider {}",
            format!("{:?}", task.role).to_ascii_lowercase(),
            task.provider.as_deref().unwrap_or("-")
        ),
        one_line(&task.subject, width),
        format!(
            "status {}  run {}",
            task_status_label(task.status),
            task.child_run_id
                .as_deref()
                .map(run_prefix)
                .unwrap_or_else(|| "-".to_string())
        ),
        format!(
            "deps {}",
            if task.depends_on.is_empty() {
                "ready".to_string()
            } else {
                task.depends_on.join(",")
            }
        ),
    ];
    if let Some(run_id) = task.child_run_id.as_deref()
        && let Ok(state) = load_run(paths, run_id)
    {
        lines.push(format!("turn {}  run-status {}", state.turn, state.status));
        lines.extend(plan_child_accounting_lines(&state));
        if let Some(trace) = latest_trace_line(&state) {
            lines.push(trace);
        }
        lines.push(format!("gate: {}", acceptance_status_value(&state)));
    } else if let Some(run_id) = task.child_run_id.as_deref() {
        lines.push(format!("child detail unavailable {}", run_prefix(run_id)));
    }
    if let Some(summary) = task.summary_path.as_ref() {
        lines.push(format!(
            "summary {}",
            paths.plan_dir(&plan.plan_id).join(summary).display()
        ));
    }
    lines
}

fn plan_child_accounting_lines(state: &deadreckon_core::PipelineState) -> Vec<String> {
    let spend = read_jsonl::<SpendRecord>(&state.run_root.join("spend.jsonl")).unwrap_or_default();
    if spend.is_empty() {
        if !provider_is_metered(state) && state.total_spend_usd == 0.0 {
            return vec!["spend not metered (subscription)  context waiting".to_string()];
        }
        return vec![format!(
            "spend ${:.6}  context waiting",
            state.total_spend_usd
        )];
    }
    let total_tokens = spend
        .iter()
        .map(|record| record.input_tokens + record.output_tokens)
        .sum::<u64>();
    let token_suffix = if total_tokens > 0 {
        format!("  tokens {}", format_count(total_tokens))
    } else {
        String::new()
    };
    vec![format!(
        "spend {}{}",
        run_spend_label(state, false),
        token_suffix
    )]
}

fn latest_trace_line(state: &deadreckon_core::PipelineState) -> Option<String> {
    let trace = read_last_jsonl::<TraceRecord>(&state.run_root.join("traces.jsonl"))?;
    Some(format!(
        "latest turn {} {} {}",
        trace.turn,
        trace.event,
        one_line(&trace.detail.to_string(), 80)
    ))
}
