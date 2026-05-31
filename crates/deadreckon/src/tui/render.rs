use super::super::*;
use pulldown_cmark::{
    CodeBlockKind, Event as MarkdownEvent, HeadingLevel, Options as MarkdownOptions,
    Parser as MarkdownParser, Tag, TagEnd,
};
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap};

pub(crate) struct PlanAttachRenderState<'a> {
    pub(crate) messages: &'a [PlanMessage],
    pub(crate) plan_events: &'a [PlanEvent],
    pub(crate) feed_events: &'a [PlanFeedEvent],
    pub(crate) selected: usize,
    pub(crate) show_hints: bool,
    pub(crate) view: AttachViewMode,
    pub(crate) visual: NarrativeVisualMode,
    pub(crate) narrative_notice: Option<&'a str>,
    pub(crate) narrative_projection: Option<&'a narrative::NarrativeProjection>,
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
            Constraint::Length(7),
            Constraint::Length(2),
        ])
        .split(area);
    let counts = plan_task_counts(plan);
    let header = vec![
        Line::from(vec![
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
                    .title("deadreckon plan")
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
            if is_selected { "*" } else { " " },
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
    if area.width >= 100 && state.visual != NarrativeVisualMode::None {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(area);
        let visual_lines = narrative::graph_ascii_lines(&projection.graph, state.visual);
        lines.retain(|line| !line.starts_with("Visual:"));
        frame.render_widget(
            List::new(lines.into_iter().map(narrative_list_item)).block(
                Block::default()
                    .title("plan narrative")
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
        frame.render_widget(
            List::new(lines.into_iter().map(narrative_list_item)).block(
                Block::default()
                    .title(format!("plan narrative / {}", state.visual.label()))
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
    view: AttachViewMode,
    visual: NarrativeVisualMode,
) -> String {
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
                let wait_hint = format!(
                    "Enter waits for child run  |  try: deadreckon fork {}",
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
                let unavailable_hint = "child detail unavailable  |  try: deadreckon list --all";
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
    if show_hints && !footer.contains("try:") {
        footer.push_str("  |  merge after fork");
    }
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
            Utc::now().format("%H:%M:%S"),
            plan_status_label(plan.status),
            plan.tasks.len()
        ),
        PlanFeedEvent::Warning { message } => {
            format!("{} warning {message}", Utc::now().format("%H:%M:%S"))
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
    let trace = read_jsonl::<TraceRecord>(&state.run_root.join("traces.jsonl"))
        .unwrap_or_default()
        .into_iter()
        .last()?;
    Some(format!(
        "latest turn {} {} {}",
        trace.turn,
        trace.event,
        one_line(&trace.detail.to_string(), 80)
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkdownBlock {
    Paragraph,
    Heading(HeadingLevel),
    Item,
}

pub(crate) fn markdown_to_tui_lines(markdown: &str) -> Vec<Line<'static>> {
    let options = MarkdownOptions::ENABLE_TABLES
        | MarkdownOptions::ENABLE_STRIKETHROUGH
        | MarkdownOptions::ENABLE_TASKLISTS;
    let parser = MarkdownParser::new_ext(markdown, options);
    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut block: Option<MarkdownBlock> = None;
    let mut inline_style = Style::default();
    let mut code_block = false;

    for event in parser {
        match event {
            MarkdownEvent::Start(Tag::Heading { level, .. }) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                block = Some(MarkdownBlock::Heading(level));
            }
            MarkdownEvent::End(TagEnd::Heading(_)) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                lines.push(Line::raw(""));
            }
            MarkdownEvent::Start(Tag::Paragraph) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                block = Some(MarkdownBlock::Paragraph);
            }
            MarkdownEvent::End(TagEnd::Paragraph) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                lines.push(Line::raw(""));
            }
            MarkdownEvent::Start(Tag::Item) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                current.push(Span::styled("  - ", Style::default().fg(Color::Cyan)));
                block = Some(MarkdownBlock::Item);
            }
            MarkdownEvent::End(TagEnd::Item) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
            }
            MarkdownEvent::Start(Tag::CodeBlock(kind)) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                let language = match kind {
                    CodeBlockKind::Fenced(language) if !language.is_empty() => {
                        format!(" {}", language)
                    }
                    _ => String::new(),
                };
                lines.push(Line::styled(
                    format!("```{language}"),
                    Style::default().fg(Color::DarkGray),
                ));
                code_block = true;
            }
            MarkdownEvent::End(TagEnd::CodeBlock) => {
                code_block = false;
                lines.push(Line::styled("```", Style::default().fg(Color::DarkGray)));
                lines.push(Line::raw(""));
            }
            MarkdownEvent::Start(Tag::Strong) => {
                inline_style = inline_style.add_modifier(Modifier::BOLD);
            }
            MarkdownEvent::End(TagEnd::Strong) => {
                inline_style = inline_style.remove_modifier(Modifier::BOLD);
            }
            MarkdownEvent::Start(Tag::Emphasis) => {
                inline_style = inline_style.add_modifier(Modifier::ITALIC);
            }
            MarkdownEvent::End(TagEnd::Emphasis) => {
                inline_style = inline_style.remove_modifier(Modifier::ITALIC);
            }
            MarkdownEvent::Start(Tag::Link { dest_url, .. }) => {
                inline_style = inline_style
                    .fg(Color::Blue)
                    .add_modifier(Modifier::UNDERLINED);
                if !dest_url.is_empty() {
                    current.push(Span::styled(
                        "",
                        Style::default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::UNDERLINED),
                    ));
                }
            }
            MarkdownEvent::End(TagEnd::Link) => {
                inline_style = Style::default();
            }
            MarkdownEvent::Text(text) => {
                if code_block {
                    for line in text.lines() {
                        lines.push(Line::styled(
                            format!("  {line}"),
                            Style::default().fg(Color::LightGreen),
                        ));
                    }
                } else {
                    current.push(Span::styled(text.into_string(), inline_style));
                }
            }
            MarkdownEvent::Code(code) => current.push(Span::styled(
                code.into_string(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            MarkdownEvent::SoftBreak => current.push(Span::raw(" ")),
            MarkdownEvent::HardBreak => {
                flush_markdown_line(&mut lines, &mut current, block);
                block = Some(MarkdownBlock::Paragraph);
            }
            MarkdownEvent::Rule => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                lines.push(Line::styled(
                    "────────────────────────────────────────",
                    Style::default().fg(Color::DarkGray),
                ));
            }
            MarkdownEvent::Html(html) | MarkdownEvent::InlineHtml(html) => current.push(
                Span::styled(html.into_string(), Style::default().fg(Color::DarkGray)),
            ),
            MarkdownEvent::InlineMath(math) => current.push(Span::styled(
                math.into_string(),
                Style::default().fg(Color::Magenta),
            )),
            MarkdownEvent::DisplayMath(math) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                lines.push(Line::styled(
                    math.into_string(),
                    Style::default().fg(Color::Magenta),
                ));
            }
            MarkdownEvent::Start(_)
            | MarkdownEvent::End(_)
            | MarkdownEvent::FootnoteReference(_) => {}
            MarkdownEvent::TaskListMarker(checked) => current.push(Span::styled(
                if checked { "[x] " } else { "[ ] " },
                Style::default().fg(Color::Cyan),
            )),
        }
    }
    flush_markdown_line(&mut lines, &mut current, block.take());
    if lines.is_empty() {
        lines.push(Line::styled(
            "Narrative docs are empty.",
            Style::default().fg(Color::Yellow),
        ));
    }
    lines
}

fn flush_markdown_line(
    lines: &mut Vec<Line<'static>>,
    current: &mut Vec<Span<'static>>,
    block: Option<MarkdownBlock>,
) {
    if current.is_empty() {
        return;
    }
    let style = match block.unwrap_or(MarkdownBlock::Paragraph) {
        MarkdownBlock::Heading(HeadingLevel::H1) => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        MarkdownBlock::Heading(HeadingLevel::H2) => Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
        MarkdownBlock::Heading(_) => Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
        MarkdownBlock::Item => Style::default().fg(Color::White),
        MarkdownBlock::Paragraph => Style::default(),
    };
    let mut spans = Vec::new();
    if matches!(block, Some(MarkdownBlock::Heading(level)) if level != HeadingLevel::H1) {
        spans.push(Span::styled("▸ ", Style::default().fg(Color::Cyan)));
    }
    spans.extend(current.drain(..).map(|span| span.patch_style(style)));
    lines.push(Line::from(spans));
}

pub(crate) fn render_attach(
    frame: &mut ratatui::Frame<'_>,
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    events: &[RunEvent],
    live: &AttachLive,
    tui_state: &AttachTuiState,
) {
    let area = frame.area();
    let layout = attach_panel_layout(area);

    let metered_provider = provider_is_metered(state);
    let top_constraints = if metered_provider {
        vec![
            Constraint::Percentage(45),
            Constraint::Percentage(25),
            Constraint::Percentage(15),
            Constraint::Percentage(15),
        ]
    } else {
        vec![
            Constraint::Percentage(66),
            Constraint::Percentage(17),
            Constraint::Percentage(17),
        ]
    };
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(top_constraints)
        .split(layout.header);
    let header = Paragraph::new(attach_header_text_for_state(
        state,
        top[0].width,
        tui_state.parent_plan.as_ref(),
    ))
    .block(Block::default().borders(Borders::ALL).title("deadreckon"));
    frame.render_widget(header, top[0]);
    if metered_provider {
        render_spend(frame, top[1], state);
        render_context(frame, top[2], spend, live);
        render_acceptance(frame, top[3], live);
    } else {
        render_context(frame, top[1], spend, live);
        render_acceptance(frame, top[2], live);
    }

    if tui_state.docs_open && state.status == RunStatus::Completed {
        render_run_docs(frame, layout.activity, state, tui_state);
    } else if tui_state.view.is_narrative() {
        render_run_narrative(
            frame,
            layout.activity,
            &RunNarrativeRenderInput {
                state,
                spend,
                traces,
                events,
                live,
                tui_state,
            },
        );
    } else {
        let trace_lines =
            attach_activity_lines_for_tui(state, spend, traces, events, live, tui_state);
        let stream_rows = layout.activity.height.saturating_sub(2) as usize;
        let trace_items = visible_items(&trace_lines, tui_state.activity_scroll, stream_rows);
        frame.render_widget(
            List::new(trace_items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(panel_border_style(
                        tui_state.focused_panel,
                        AttachPanel::Activity,
                    ))
                    .title(panel_title(
                        "tool calls / provider activity",
                        tui_state.focused_panel == AttachPanel::Activity,
                        tui_state.activity_scroll,
                        stream_rows,
                        trace_lines.len(),
                    )),
            ),
            layout.activity,
        );
    }
    render_live_files(frame, layout.files, live, tui_state);
    render_processes(frame, layout.processes, live, tui_state);
    let tick = (Utc::now().timestamp_millis() / 180).max(0) as usize;
    let status_line = deadreckoning_status_line(
        state,
        &turn_timer(events, spend, traces, state),
        layout.footer.width,
        tick,
    );
    frame.render_widget(
        Paragraph::new(vec![
            status_line,
            Line::from(footer_for_state(state, tui_state)),
        ]),
        layout.footer,
    );
}

pub(crate) fn provider_is_metered(state: &deadreckon_core::PipelineState) -> bool {
    !state
        .provider
        .as_deref()
        .is_some_and(|provider| provider.starts_with("cli:") || provider.starts_with("import:"))
}

#[cfg(test)]
pub(crate) fn attach_header_text(state: &deadreckon_core::PipelineState, width: u16) -> String {
    attach_header_text_for_state(state, width, None)
}

fn attach_header_text_for_state(
    state: &deadreckon_core::PipelineState,
    width: u16,
    parent_plan: Option<&AttachParentPlan>,
) -> String {
    let path_label = if state.promoted_library_dir.is_some() {
        "artifact"
    } else {
        "working"
    };
    let chain_prefix = chain_context_line_for_working(&state.working_dir)
        .ok()
        .flatten()
        .unwrap_or_default();
    let plan_prefix = parent_plan
        .map(|parent| format!("plan {} / {}", run_prefix(&parent.plan_id), parent.task_id))
        .unwrap_or_default();
    let context = [plan_prefix, chain_prefix]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("  |  ");
    let context_prefix = if context.is_empty() {
        String::new()
    } else {
        format!("{context}  |  ")
    };
    let usable_width = usize::from(width).saturating_sub(4);
    let run_line = one_line(
        &format!(
            "{}run {}  provider {}  sandbox {}",
            context_prefix,
            run_prefix(&state.run_id),
            state.provider.as_deref().unwrap_or("-"),
            state.sandbox
        ),
        usable_width,
    );
    let goal_line = one_line(&format!("goal {}", state.goal), usable_width);
    let path_line = one_line(
        &format!("{} {}", path_label, state.working_dir.display()),
        usable_width,
    );
    format!("{run_line}\n{goal_line}\n{path_line}")
}

fn footer_for_state(state: &deadreckon_core::PipelineState, tui_state: &AttachTuiState) -> String {
    let chain_suffix = if read_chain_step_marker(&state.working_dir)
        .ok()
        .flatten()
        .is_some()
    {
        "  [c] Chain"
    } else {
        ""
    };
    if state.run_root.join("abandoned.json").exists() {
        let footer = format!(
            "worktree cleaned or abandoned  |  q detach  |  deadreckon status/list{chain_suffix}"
        );
        return parent_plan_footer(footer, tui_state.parent_plan.as_ref());
    }
    let base = if tui_state.show_completion_actions && state.status == RunStatus::Completed {
        if is_worktree_run(state) {
            if tui_state.docs_open {
                "[d] Activity  [a] Apply  [b] Abandon  [s] Show  |  Tab focus  j/k scroll  q detach"
            } else if tui_state.view.is_narrative() {
                "[n] Activity  [v] Visual  [r] Refresh  [d] Docs  [a] Apply  [b] Abandon  [s] Show  |  q detach"
            } else {
                "[n] Narrative  [d] Docs  [a] Apply  [b] Abandon  [s] Show  |  Tab focus  j/k scroll  q detach"
            }
            .to_string()
        } else {
            if tui_state.docs_open {
                "[d] Activity  [m] Materialize  [e] Extend  [s] Show  |  Tab focus  j/k scroll  q detach"
            } else if tui_state.view.is_narrative() {
                "[n] Activity  [v] Visual  [r] Refresh  [d] Docs  [m] Materialize  [e] Extend  [s] Show  |  q detach"
            } else {
                "[n] Narrative  [d] Docs  [m] Materialize  [e] Extend  [s] Show  |  Tab focus  j/k scroll  q detach"
            }
            .to_string()
        }
    } else if tui_state.view.is_narrative() {
        format!(
            "[n] Activity  [v] Visual={}  [r] Refresh  |  Tab focus  j/k scroll  q detach",
            tui_state.visual.label()
        )
    } else {
        "Detach: q Esc Ctrl-D  |  [n] Narrative  |  Focus: Tab  |  Scroll: j/k Up/Down PgUp/PgDn mouse".to_string()
    };
    parent_plan_footer(
        format!("{base}{chain_suffix}"),
        tui_state.parent_plan.as_ref(),
    )
}

fn parent_plan_footer(footer: String, parent_plan: Option<&AttachParentPlan>) -> String {
    let Some(parent_plan) = parent_plan else {
        return footer;
    };
    let footer = footer
        .replace(
            "q/Esc/Ctrl-D detach",
            "b/Backspace/q/Esc/Ctrl-D back to plan",
        )
        .replace(
            "Detach: q Esc Ctrl-D",
            "Back to plan: b Backspace q Esc Ctrl-D",
        )
        .replace("q detach", "b/Backspace/q back to plan");
    let parent_label = format!(
        "parent plan {} {}",
        run_prefix(&parent_plan.plan_id),
        parent_plan.task_id
    );
    match footer.split_once("  |  ") {
        Some((lead, rest)) => format!("{lead}  |  {parent_label}  |  {rest}"),
        None => format!("{footer}  |  {parent_label}"),
    }
}

fn deadreckoning_status_line(
    state: &deadreckon_core::PipelineState,
    turn_label: &str,
    width: u16,
    tick: usize,
) -> Line<'static> {
    let text = deadreckoning_status_text(state, turn_label, width, tick);
    let split = text.find("  ").unwrap_or(text.len());
    let (prefix, rest) = text.split_at(split);
    Line::from(vec![
        Span::styled(
            prefix.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(rest.to_string(), Style::default().fg(Color::Blue)),
    ])
}

pub(crate) fn deadreckoning_status_text(
    state: &deadreckon_core::PipelineState,
    turn_label: &str,
    width: u16,
    tick: usize,
) -> String {
    let status = run_status_label(state.status);
    let phase = state
        .active_phase()
        .map(|phase| phase.name.as_str())
        .unwrap_or("phase");
    let prefix = format!("deadreckoning {status}  turn {turn_label}  {phase}  ");
    let max_width = usize::from(width).saturating_sub(1);
    let course_width = max_width.saturating_sub(prefix.chars().count()).max(8);
    let mut text = format!("{prefix}{}", deadreckoning_course_ascii(course_width, tick));
    if max_width > 0 {
        text = truncate_text(&text, max_width);
    }
    text
}

fn render_spend(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &deadreckon_core::PipelineState,
) {
    let cap = state.max_spend_usd.unwrap_or({
        if state.total_spend_usd <= 0.0 {
            1.0
        } else {
            state.total_spend_usd
        }
    });
    let spend_ratio = (state.total_spend_usd / cap).clamp(0.0, 1.0);
    let title = if spend_ratio >= 0.6 {
        format!("{:.0}% of budget", spend_ratio * 100.0)
    } else {
        "spend".to_string()
    };
    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title(title))
            .gauge_style(Style::default().fg(meter_color(spend_ratio, state)))
            .ratio(spend_ratio)
            .label(spend_meter_label(state, cap, spend_ratio)),
        area,
    );
}

fn spend_meter_label(state: &deadreckon_core::PipelineState, cap: f64, spend_ratio: f64) -> String {
    if !provider_is_metered(state) && state.total_spend_usd == 0.0 {
        return format!(
            "not metered (subscription)  wall {:.0}s",
            state.total_wall_seconds.max(0.0)
        );
    }
    let base = format!("${:.6} / ${:.6}", state.total_spend_usd, cap);
    if spend_ratio >= 0.6 {
        format!("{base}  ({:.0}% of budget)", spend_ratio * 100.0)
    } else {
        base
    }
}

fn render_context(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    spend: &[SpendRecord],
    live: &AttachLive,
) {
    let (token_total, context_window) = context_totals(spend, live);
    let context_ratio = if context_window == 0 {
        0.0
    } else {
        token_total as f64 / context_window as f64
    };
    let detail = if token_total == 0 {
        format!(
            "waiting for telemetry\n{} window",
            format_count(context_window)
        )
    } else if context_ratio >= 1.0 {
        format!(
            "{} used\n{} window\n{context_ratio:.1}x cumulative",
            format_count(token_total),
            format_count(context_window)
        )
    } else {
        format!(
            "{} used\n{} window\n{:.0}% of window",
            format_count(token_total),
            format_count(context_window),
            context_ratio * 100.0
        )
    };
    frame.render_widget(
        Paragraph::new(detail)
            .block(Block::default().borders(Borders::ALL).title("context"))
            .style(Style::default().fg(threshold_color(context_ratio.clamp(0.0, 1.0))))
            .alignment(Alignment::Center),
        area,
    );
}

fn render_acceptance(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    live: &AttachLive,
) {
    let acceptance = &live.acceptance;
    let color = acceptance_color(acceptance.status);
    let latest = acceptance
        .latest_detail
        .as_deref()
        .map(|detail| one_line(detail, usize::from(area.width).saturating_sub(4)));
    let detail = match acceptance.status {
        AcceptanceUiStatus::DefaultGate => {
            "default gate\ninferred checks\nno project spec".to_string()
        }
        AcceptanceUiStatus::Configured => format!(
            "configured\n{} checks\n{}",
            acceptance.total,
            latest.as_deref().unwrap_or("waiting for verify")
        ),
        AcceptanceUiStatus::Running => format!(
            "running\n{} / {} checked\n{}",
            acceptance.completed,
            acceptance.total,
            latest.as_deref().unwrap_or("checking criteria")
        ),
        AcceptanceUiStatus::Passed => format!(
            "passed\n{} / {} checks\n{}",
            acceptance.passed,
            acceptance.total,
            latest.as_deref().unwrap_or("dr-gate accepted")
        ),
        AcceptanceUiStatus::Failed => format!(
            "failed\n{} pass  {} fail\n{}",
            acceptance.passed,
            acceptance.failed,
            latest.as_deref().unwrap_or("required check failed")
        ),
    };
    frame.render_widget(
        Paragraph::new(detail)
            .block(Block::default().borders(Borders::ALL).title("acceptance"))
            .style(Style::default().fg(color))
            .alignment(Alignment::Center),
        area,
    );
}

fn acceptance_color(status: AcceptanceUiStatus) -> Color {
    match status {
        AcceptanceUiStatus::DefaultGate => ui::TUI_PALETTE.acceptance_default,
        AcceptanceUiStatus::Configured => ui::TUI_PALETTE.acceptance_configured,
        AcceptanceUiStatus::Running => ui::TUI_PALETTE.acceptance_running,
        AcceptanceUiStatus::Passed => ui::TUI_PALETTE.acceptance_passed,
        AcceptanceUiStatus::Failed => ui::TUI_PALETTE.acceptance_failed,
    }
}

fn render_live_files(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    live: &AttachLive,
    tui_state: &AttachTuiState,
) {
    let lines = live_file_lines(live);
    let rows = area.height.saturating_sub(2) as usize;
    let items = visible_items(&lines, tui_state.files_scroll, rows);
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(panel_border_style(
                    tui_state.focused_panel,
                    AttachPanel::Files,
                ))
                .title(panel_title(
                    &format!(
                        "live files  {} files  {}",
                        live.file_count,
                        format_bytes(live.total_bytes)
                    ),
                    tui_state.focused_panel == AttachPanel::Files,
                    tui_state.files_scroll,
                    rows,
                    lines.len(),
                )),
        ),
        area,
    );
}

fn render_processes(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    live: &AttachLive,
    tui_state: &AttachTuiState,
) {
    let lines = process_lines(live);
    let rows = area.height.saturating_sub(2) as usize;
    let items = visible_items(&lines, tui_state.processes_scroll, rows);
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(panel_border_style(
                    tui_state.focused_panel,
                    AttachPanel::Processes,
                ))
                .title(panel_title(
                    "processes",
                    tui_state.focused_panel == AttachPanel::Processes,
                    tui_state.processes_scroll,
                    rows,
                    lines.len(),
                )),
        ),
        area,
    );
}

fn turn_timer(
    events: &[RunEvent],
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    state: &deadreckon_core::PipelineState,
) -> String {
    let Some(started) = events.iter().rev().find(|event| {
        matches!(
            event.event,
            deadreckon_core::RunEventKind::TurnStarted { .. }
        )
    }) else {
        return "-".to_string();
    };
    if state.status == RunStatus::Executing {
        let elapsed = Utc::now()
            .signed_duration_since(started.timestamp)
            .num_seconds()
            .max(0);
        return format!("{elapsed}s running");
    }
    if let Some(done_at) = events.iter().rev().find_map(|event| match event.event {
        deadreckon_core::RunEventKind::RunCompleted { .. } => Some(event.timestamp),
        _ => None,
    }) {
        let elapsed = done_at
            .signed_duration_since(started.timestamp)
            .num_seconds()
            .max(0);
        return format!("{elapsed}s done");
    }
    if let Some(seconds) = spend
        .iter()
        .rev()
        .find_map(|record| record.wall_time_seconds)
    {
        return format!("{:.0}s done", seconds.max(0.0));
    }
    if let Some(ms) = traces.iter().rev().find_map(|record| record.latency_ms) {
        return format!("{:.0}s done", ms as f64 / 1000.0);
    }
    if let Some(done_at) = events
        .iter()
        .rev()
        .find(|event| {
            !matches!(
                event.event,
                deadreckon_core::RunEventKind::TurnStarted { .. }
            )
        })
        .map(|event| event.timestamp)
    {
        let elapsed = done_at
            .signed_duration_since(started.timestamp)
            .num_seconds()
            .max(0);
        return format!("{elapsed}s done");
    }
    "done".to_string()
}

pub(crate) fn meter_color(ratio: f64, state: &deadreckon_core::PipelineState) -> Color {
    if state.pause_reason.as_deref() == Some("spend cap reached") {
        ui::TUI_PALETTE.spend_pause_cap
    } else {
        threshold_color(ratio)
    }
}

pub(crate) fn threshold_color(ratio: f64) -> Color {
    if ratio >= 0.8 {
        ui::TUI_PALETTE.spend_high
    } else if ratio >= 0.6 {
        ui::TUI_PALETTE.spend_mid
    } else {
        ui::TUI_PALETTE.spend_low
    }
}

pub(crate) fn context_totals(spend: &[SpendRecord], live: &AttachLive) -> (u64, u64) {
    let token_total = live.provider_context_tokens.unwrap_or_else(|| {
        spend
            .iter()
            .map(|record| record.input_tokens + record.output_tokens)
            .sum::<u64>()
    });
    let context_window = live.provider_context_window.unwrap_or(200_000).max(1);
    (token_total, context_window)
}

fn render_turn_summary(spend: &[SpendRecord], show_cost: bool) -> Vec<String> {
    if spend.is_empty() {
        vec!["provider turn in progress; results land when the provider exits".to_string()]
    } else {
        spend
            .iter()
            .rev()
            .take(3)
            .map(|record| {
                let tokens = record.input_tokens + record.output_tokens;
                if show_cost {
                    format!(
                        "turn {}  {}  {} tokens  ${:.6}",
                        record.turn, record.model, tokens, record.cost_usd
                    )
                } else if let Some(seconds) = record.wall_time_seconds {
                    format!(
                        "turn {}  {}  {} tokens  {:.0}s wall",
                        record.turn,
                        record.model,
                        tokens,
                        seconds.max(0.0)
                    )
                } else {
                    format!("turn {}  {}  {} tokens", record.turn, record.model, tokens)
                }
            })
            .collect()
    }
}

pub(crate) fn attach_activity_lines_for_tui(
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    events: &[RunEvent],
    live: &AttachLive,
    tui_state: &AttachTuiState,
) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(notice) = tui_state.post_action_notice.as_ref() {
        lines.extend(notice.lines());
        lines.push(String::new());
    }
    lines.extend(attach_activity_lines(state, spend, traces, events, live));
    lines
}

fn attach_activity_lines(
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    events: &[RunEvent],
    live: &AttachLive,
) -> Vec<String> {
    let metered_provider = provider_is_metered(state);
    let mut lines = render_turn_summary(spend, metered_provider);
    lines.extend(acceptance_activity_lines(&live.acceptance));
    if state.status == RunStatus::Executing && live.file_count > 0 {
        lines.push(format!(
            "live working tree: {} files, latest changes visible before provider exit",
            live.file_count
        ));
    }
    lines.extend(live.provider_activity.iter().rev().cloned());
    lines.extend(
        events
            .iter()
            .rev()
            .map(|event| event_line(event, metered_provider)),
    );
    lines.extend(traces.iter().rev().map(|record| {
        format!(
            "trace turn {}  {}  {:?}ms",
            record.turn, record.event, record.latency_ms
        )
    }));
    lines
}

pub(crate) fn acceptance_activity_lines(acceptance: &AcceptanceLive) -> Vec<String> {
    match acceptance.status {
        AcceptanceUiStatus::DefaultGate | AcceptanceUiStatus::Configured => Vec::new(),
        AcceptanceUiStatus::Running => {
            let mut lines = vec![format!(
                "acceptance running: {} / {} checked",
                acceptance.completed, acceptance.total
            )];
            lines.extend(acceptance.progress_lines.iter().cloned());
            lines.push(String::new());
            lines
        }
        AcceptanceUiStatus::Passed => {
            let mut lines = vec![format!(
                "acceptance passed: {} / {} checks",
                acceptance.passed, acceptance.total
            )];
            lines.extend(acceptance.progress_lines.iter().take(4).cloned());
            lines.push(String::new());
            lines
        }
        AcceptanceUiStatus::Failed => {
            let mut lines = vec![format!(
                "acceptance failed: {} required failures, {} / {} passed",
                acceptance.required_failed, acceptance.passed, acceptance.total
            )];
            lines.extend(acceptance.progress_lines.iter().cloned());
            lines.push(String::new());
            lines
        }
    }
}

pub(crate) fn live_file_lines(live: &AttachLive) -> Vec<String> {
    if !live.working_dir_exists {
        return vec!["working tree was removed after cleanup".to_string()];
    }
    if live.files.is_empty() {
        return vec!["no files yet".to_string()];
    }
    let mut lines = Vec::new();
    lines.extend(live.files.iter().map(|file| {
        format!(
            "{:>7} {:>8}  {}",
            format_age(file.modified_at),
            format_bytes(file.bytes),
            file.path
        )
    }));
    if live.file_count > live.files.len() {
        lines.push(format!(
            "... {} more files not shown",
            live.file_count - live.files.len()
        ));
    }
    lines
}

pub(crate) fn process_lines(live: &AttachLive) -> Vec<String> {
    if live.pids.is_empty() {
        vec!["no supervised pids".to_string()]
    } else {
        live.pids
            .iter()
            .map(|pid| {
                let status = if pid.alive { "alive" } else { "dead" };
                format!("{} {} {}", pid.pid, status, pid.command)
            })
            .collect()
    }
}

pub(crate) fn visible_items(
    lines: &[String],
    offset: usize,
    rows: usize,
) -> Vec<ListItem<'static>> {
    lines
        .iter()
        .skip(offset.min(lines.len()))
        .take(rows)
        .map(|line| ListItem::new(line.clone()))
        .collect()
}

pub(crate) fn visible_narrative_items(
    lines: &[String],
    offset: usize,
    rows: usize,
) -> Vec<ListItem<'static>> {
    lines
        .iter()
        .skip(offset.min(lines.len()))
        .take(rows)
        .cloned()
        .map(narrative_list_item)
        .collect()
}

pub(crate) fn narrative_list_item(line: String) -> ListItem<'static> {
    let style = if line.starts_with("[done]") || line.contains("[success]") {
        Style::default().fg(Color::Green)
    } else if line.starts_with("[risk]")
        || line.starts_with("[stale]")
        || line.contains("[warning]")
        || line.contains("failed")
    {
        Style::default().fg(Color::Yellow)
    } else if line.starts_with("[blocked]") || line.contains("[danger]") {
        Style::default().fg(Color::Red)
    } else if line.starts_with("Current work")
        || line.starts_with("Architecture")
        || line.starts_with("Agents")
        || line.starts_with("Coordination")
        || line.starts_with("Risks")
        || line.starts_with("Next likely")
        || line.starts_with("Visual:")
        || line.starts_with("Evidence")
    {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if line.starts_with("  ->") || line.contains(" -> ") {
        Style::default().fg(Color::LightCyan)
    } else if line.starts_with("- ") {
        Style::default().fg(Color::White)
    } else {
        Style::default()
    };
    ListItem::new(Line::styled(line, style))
}

pub(crate) fn panel_border_style(focused: AttachPanel, panel: AttachPanel) -> Style {
    if focused == panel {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    }
}

pub(crate) fn panel_title(
    title: &str,
    focused: bool,
    offset: usize,
    rows: usize,
    total: usize,
) -> String {
    let marker = if focused { "*" } else { " " };
    if total <= rows || total == 0 {
        format!("{marker}{title}")
    } else {
        let first = offset.saturating_add(1).min(total);
        let last = offset.saturating_add(rows).min(total);
        format!("{marker}{title} {first}-{last}/{total}")
    }
}
