use super::super::*;
use super::panes::footer::footer;
use pulldown_cmark::{
    CodeBlockKind, Event as MarkdownEvent, HeadingLevel, Options as MarkdownOptions,
    Parser as MarkdownParser, Tag, TagEnd,
};
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap};

pub(crate) fn render_run_docs(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &deadreckon_core::PipelineState,
    tui_state: &AttachTuiState,
) {
    let lines = render_markdown_doc_lines(state);
    let rows = area.height.saturating_sub(2) as usize;
    frame.render_widget(
        Paragraph::new(lines.clone())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(panel_border_style(
                        tui_state.focused_panel,
                        AttachPanel::Activity,
                    ))
                    .title(panel_title(
                        "run docs / narrative",
                        tui_state.focused_panel == AttachPanel::Activity,
                        tui_state.docs_scroll,
                        rows,
                        lines.len(),
                    )),
            )
            .wrap(Wrap { trim: false })
            .scroll((tui_state.docs_scroll as u16, 0)),
        area,
    );
}

pub(crate) struct RunNarrativeRenderInput<'a> {
    pub(crate) state: &'a deadreckon_core::PipelineState,
    pub(crate) spend: &'a [SpendRecord],
    pub(crate) traces: &'a [TraceRecord],
    pub(crate) events: &'a [RunEvent],
    pub(crate) live: &'a AttachLive,
    pub(crate) tui_state: &'a AttachTuiState,
}

/// Minimum terminal width at which the narrative view splits into a body +
/// visual-graph column. Run and plan share this single breakpoint.
pub(crate) const NARRATIVE_SPLIT_WIDTH: u16 = 100;

/// Height of the plan-attach narrative/activity row (`vertical[2]`). The inner
/// list is two rows shorter (top/bottom border); the attach loop relies on this
/// to clamp the narrative scroll consistently with the renderer.
pub(crate) const PLAN_NARRATIVE_AREA_HEIGHT: u16 = 7;

pub(crate) fn render_run_narrative(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    input: &RunNarrativeRenderInput<'_>,
) {
    let tui_state = input.tui_state;
    let rows = area.height.saturating_sub(2) as usize;
    let projection = run_narrative_projection_for_render(input);
    let mut lines = run_narrative_lines_from_projection(&projection, tui_state);
    if area.width >= NARRATIVE_SPLIT_WIDTH && tui_state.visual != NarrativeVisualMode::None {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(area);
        let visual_lines = narrative::graph_ascii_lines(&projection.graph, tui_state.visual);
        lines.retain(|line| !line.starts_with("Visual:"));
        let visible = visible_narrative_items(&lines, tui_state.narrative_scroll, rows);
        frame.render_widget(
            List::new(visible).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(panel_border_style(
                        tui_state.focused_panel,
                        AttachPanel::Activity,
                    ))
                    .title(panel_title(
                        "narrative",
                        tui_state.focused_panel == AttachPanel::Activity,
                        tui_state.narrative_scroll,
                        rows,
                        lines.len(),
                    )),
            ),
            chunks[0],
        );
        frame.render_widget(
            List::new(visual_lines.into_iter().map(narrative_list_item)).block(
                Block::default()
                    .title(format!("visual {}", tui_state.visual.label()))
                    .borders(Borders::ALL),
            ),
            chunks[1],
        );
    } else {
        let visible = visible_narrative_items(&lines, tui_state.narrative_scroll, rows);
        frame.render_widget(
            List::new(visible).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(panel_border_style(
                        tui_state.focused_panel,
                        AttachPanel::Activity,
                    ))
                    .title(panel_title(
                        &format!("narrative / {}", tui_state.visual.label()),
                        tui_state.focused_panel == AttachPanel::Activity,
                        tui_state.narrative_scroll,
                        rows,
                        lines.len(),
                    )),
            ),
            area,
        );
    }
}

pub(crate) fn run_narrative_lines(
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    events: &[RunEvent],
    live: &AttachLive,
    tui_state: &AttachTuiState,
) -> Vec<String> {
    let projection = run_narrative_projection_for_render(&RunNarrativeRenderInput {
        state,
        spend,
        traces,
        events,
        live,
        tui_state,
    });
    run_narrative_lines_from_projection(&projection, tui_state)
}

fn run_narrative_lines_from_projection(
    projection: &narrative::NarrativeProjection,
    tui_state: &AttachTuiState,
) -> Vec<String> {
    let mut lines = narrative::narrative_plain_lines(projection, tui_state.visual);
    if let Some(notice) = tui_state.narrative_notice.as_ref() {
        lines.insert(2, format!("[fresh] {notice}"));
    }
    lines
}

pub(crate) fn run_narrative_projection(
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    events: &[RunEvent],
    live: &AttachLive,
    tui_state: &AttachTuiState,
) -> Result<narrative::NarrativeProjection> {
    ensure_run_narrative_projection(&RunNarrativeRenderInput {
        state,
        spend,
        traces,
        events,
        live,
        tui_state,
    })
}

fn run_narrative_projection_for_render(
    input: &RunNarrativeRenderInput<'_>,
) -> narrative::NarrativeProjection {
    if let Some(projection) = input.tui_state.narrative_projection.as_ref() {
        return projection.clone();
    }
    build_run_narrative_projection(input)
}

pub(crate) fn build_run_narrative_projection(
    input: &RunNarrativeRenderInput<'_>,
) -> narrative::NarrativeProjection {
    narrative::build_run_projection(&run_narrative_input(input))
}

pub(crate) fn ensure_run_narrative_projection(
    input: &RunNarrativeRenderInput<'_>,
) -> Result<narrative::NarrativeProjection> {
    narrative::ensure_run_projection(&run_narrative_input(input))
}

fn run_narrative_input<'a>(
    input: &'a RunNarrativeRenderInput<'a>,
) -> narrative::RunNarrativeInput<'a> {
    let state = input.state;
    let live = input.live;
    let tui_state = input.tui_state;
    narrative::RunNarrativeInput {
        state,
        spend: input.spend,
        traces: input.traces,
        events: input.events,
        live_files: live
            .files
            .iter()
            .map(|file| narrative::LiveFileFact {
                path: file.path.clone(),
                bytes: file.bytes,
                modified_at: file.modified_at,
            })
            .collect(),
        file_count: live.file_count,
        total_bytes: live.total_bytes,
        acceptance_summary: acceptance_narrative_summary(&live.acceptance),
        provider_activity: &live.provider_activity,
        parent_plan: tui_state
            .parent_plan
            .as_ref()
            .map(|parent| narrative::ParentPlanFact {
                plan_id: parent.plan_id.clone(),
                task_id: parent.task_id.clone(),
            }),
    }
}

pub(crate) fn run_narrative_projection_signature(input: &RunNarrativeRenderInput<'_>) -> u64 {
    let mut hasher = DefaultHasher::new();
    input.state.run_id.hash(&mut hasher);
    format!("{:?}", input.state.status).hash(&mut hasher);
    input.state.turn.hash(&mut hasher);
    input
        .state
        .active_phase()
        .map(|phase| &phase.name)
        .hash(&mut hasher);
    input.spend.len().hash(&mut hasher);
    input
        .spend
        .last()
        .map(|record| record.total_cost_usd.to_bits())
        .hash(&mut hasher);
    input.traces.len().hash(&mut hasher);
    input
        .traces
        .last()
        .map(|record| (&record.turn, &record.event))
        .hash(&mut hasher);
    input.events.len().hash(&mut hasher);
    input.live.file_count.hash(&mut hasher);
    input.live.total_bytes.hash(&mut hasher);
    input
        .live
        .files
        .first()
        .map(|file| {
            (
                &file.path,
                file.bytes,
                file.modified_at.map(|date| date.timestamp_millis()),
            )
        })
        .hash(&mut hasher);
    input.live.provider_activity.len().hash(&mut hasher);
    input.live.provider_activity.last().hash(&mut hasher);
    format!("{:?}", input.live.acceptance.status).hash(&mut hasher);
    input.live.acceptance.completed.hash(&mut hasher);
    input.live.acceptance.failed.hash(&mut hasher);
    input.live.acceptance.latest_detail.hash(&mut hasher);
    input
        .tui_state
        .parent_plan
        .as_ref()
        .map(|parent| (&parent.plan_id, &parent.task_id))
        .hash(&mut hasher);
    hasher.finish()
}

fn acceptance_narrative_summary(acceptance: &AcceptanceLive) -> String {
    match acceptance.status {
        AcceptanceUiStatus::DefaultGate => "default gate".to_string(),
        AcceptanceUiStatus::Configured => format!("configured {} check(s)", acceptance.total),
        AcceptanceUiStatus::Running => {
            format!(
                "running {}/{} check(s)",
                acceptance.completed, acceptance.total
            )
        }
        AcceptanceUiStatus::Passed => {
            format!("passed {}/{} check(s)", acceptance.passed, acceptance.total)
        }
        AcceptanceUiStatus::Failed => format!(
            "failed {} required, {} passed of {}",
            acceptance.required_failed, acceptance.passed, acceptance.total
        ),
    }
}

pub(crate) fn render_markdown_doc_lines(
    state: &deadreckon_core::PipelineState,
) -> Vec<Line<'static>> {
    let Some(path) = doc_path_for_kind(&state.working_dir, DocKind::Narrative) else {
        return vec![Line::styled(
            "No narrative docs found for this run.",
            Style::default().fg(Color::Yellow),
        )];
    };
    match fs::read_to_string(&path) {
        Ok(raw) => markdown_to_tui_lines(&raw),
        Err(err) => vec![Line::styled(
            format!("Unable to read {}: {err}", path.display()),
            Style::default().fg(Color::Red),
        )],
    }
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

pub(crate) fn attach_header_text_for_state(
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
        .map(|parent| {
            let campaign = parent
                .campaign_parent
                .as_ref()
                .map(|campaign| {
                    format!(
                        "campaign {} / {} / ",
                        run_prefix(&campaign.campaign_id),
                        campaign.sub_id
                    )
                })
                .unwrap_or_default();
            format!(
                "{campaign}plan {} / {}",
                run_prefix(&parent.plan_id),
                parent.task_id
            )
        })
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

pub(crate) fn footer_for_state(
    state: &deadreckon_core::PipelineState,
    tui_state: &AttachTuiState,
) -> String {
    if let Some(action) = tui_state.pending_confirm {
        return footer(&[
            (format!("confirm {}?", action.label()), String::new()),
            ("y".to_string(), "run".to_string()),
            ("any other key".to_string(), "cancel".to_string()),
        ]);
    }
    let mut items: Vec<(String, String)> = Vec::new();
    let push = |items: &mut Vec<(String, String)>, pairs: &[(&str, &str)]| {
        items.extend(pairs.iter().map(|(k, l)| (k.to_string(), l.to_string())));
    };

    // Detach (or, for a child run attached from a plan, back-to-plan + the parent
    // breadcrumb) comes first so the exit affordance and parent context stay
    // visible even when a narrow terminal truncates the footer tail.
    if let Some(parent_plan) = tui_state.parent_plan.as_ref() {
        items.push((
            "b/Backspace/q/Esc/Ctrl-D".to_string(),
            "back to plan".to_string(),
        ));
        let campaign = parent_plan
            .campaign_parent
            .as_ref()
            .map(|campaign| {
                format!(
                    "campaign {} / {} / ",
                    run_prefix(&campaign.campaign_id),
                    campaign.sub_id
                )
            })
            .unwrap_or_default();
        items.push((
            "parent".to_string(),
            format!(
                "{campaign}plan {} {}",
                run_prefix(&parent_plan.plan_id),
                parent_plan.task_id
            ),
        ));
    } else {
        items.push(("q/Esc/Ctrl-D".to_string(), "detach".to_string()));
    }

    if state.run_root.join("abandoned.json").exists() {
        push(
            &mut items,
            &[
                ("worktree", "cleaned or abandoned"),
                ("deadreckon", "status/list"),
            ],
        );
    } else if tui_state.show_completion_actions && state.status == RunStatus::Completed {
        let actions: &[(&str, &str)] = if is_worktree_run(state) {
            if tui_state.docs_open {
                &[
                    ("[d]", "Activity"),
                    ("[a]", "Apply"),
                    ("[x]", "Abandon"),
                    ("[s]", "Show"),
                    ("Tab", "focus"),
                    ("j/k", "scroll"),
                ]
            } else if tui_state.view.is_narrative() {
                &[
                    ("[n]", "Activity"),
                    ("[v]", "Visual"),
                    ("[r]", "Refresh"),
                    ("[d]", "Docs"),
                    ("[a]", "Apply"),
                    ("[x]", "Abandon"),
                    ("[s]", "Show"),
                ]
            } else {
                &[
                    ("[n]", "Narrative"),
                    ("[d]", "Docs"),
                    ("[a]", "Apply"),
                    ("[x]", "Abandon"),
                    ("[s]", "Show"),
                    ("Tab", "focus"),
                    ("j/k", "scroll"),
                ]
            }
        } else if tui_state.docs_open {
            &[
                ("[d]", "Activity"),
                ("[m]", "Materialize"),
                ("[e]", "Extend"),
                ("[s]", "Show"),
                ("Tab", "focus"),
                ("j/k", "scroll"),
            ]
        } else if tui_state.view.is_narrative() {
            &[
                ("[n]", "Activity"),
                ("[v]", "Visual"),
                ("[r]", "Refresh"),
                ("[d]", "Docs"),
                ("[m]", "Materialize"),
                ("[e]", "Extend"),
                ("[s]", "Show"),
            ]
        } else {
            &[
                ("[n]", "Narrative"),
                ("[d]", "Docs"),
                ("[m]", "Materialize"),
                ("[e]", "Extend"),
                ("[s]", "Show"),
                ("Tab", "focus"),
                ("j/k", "scroll"),
            ]
        };
        push(&mut items, actions);
    } else if tui_state.why_open {
        push(
            &mut items,
            &[("[w]", "Activity"), ("Tab", "focus"), ("j/k", "scroll")],
        );
    } else if tui_state.view.is_narrative() {
        push(&mut items, &[("[n]", "Activity")]);
        items.push((
            "[v]".to_string(),
            format!("Visual={}", tui_state.visual.label()),
        ));
        push(
            &mut items,
            &[("[r]", "Refresh"), ("Tab", "focus"), ("j/k", "scroll")],
        );
    } else {
        push(
            &mut items,
            &[
                ("[n]", "Narrative"),
                ("[w]", "Why"),
                ("Tab", "focus"),
                ("j/k Up/Down PgUp/PgDn", "scroll"),
            ],
        );
    }

    if read_chain_step_marker(&state.working_dir)
        .ok()
        .flatten()
        .is_some()
    {
        push(&mut items, &[("[c]", "Chain")]);
    }

    // Detach vs back-to-plan is structural (no string-replace hack): a child run
    // attached from a plan shows the parent label and a back affordance instead.
    items.push(("?".to_string(), "help".to_string()));
    footer(&items)
}

/// Which attach surface the `?` help overlay describes. Uniform chrome,
/// mode-specific content: every surface gets the same overlay frame and close
/// semantics, with only the binding list varying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachHelpMode {
    Run,
    Plan,
    Campaign,
    Chain,
}

impl AttachHelpMode {
    fn title(self) -> &'static str {
        match self {
            Self::Run => "run attach keys",
            Self::Plan => "plan attach keys",
            Self::Campaign => "campaign attach keys",
            Self::Chain => "chain attach keys",
        }
    }
}

/// The complete binding list per attach surface — the footer shows the
/// load-bearing subset and truncates on narrow terminals; this overlay is the
/// full reference, one keystroke away.
pub(crate) fn help_overlay_lines(mode: AttachHelpMode) -> Vec<(&'static str, &'static str)> {
    match mode {
        AttachHelpMode::Run => vec![
            ("q / Esc / Ctrl-D", "detach (run keeps going)"),
            ("b / Backspace", "back to plan (when drilled in)"),
            ("n", "toggle narrative / activity panels"),
            ("v", "cycle visual map mode (narrative view)"),
            ("r", "refresh narrative"),
            ("Tab / Shift-Tab", "switch focused panel"),
            ("j k / Up Down", "scroll focused panel"),
            ("PgUp / PgDn", "page"),
            ("Home End / g G", "jump to start / end"),
            ("c", "open chain context (chain-step runs)"),
            ("a", "apply completed run (y confirms)"),
            ("x", "abandon completed run (y confirms)"),
            ("m / e", "materialize / extend completed run"),
            ("s / d", "show details / toggle docs"),
            ("?", "toggle this help"),
        ],
        AttachHelpMode::Plan => vec![
            ("q / Esc / Ctrl-D", "detach (plan keeps going)"),
            ("b / Backspace", "back to campaign (when drilled in)"),
            ("Enter", "open the selected child run"),
            ("n", "toggle narrative / task cards"),
            ("v", "cycle visual map mode (narrative view)"),
            ("r", "refresh narrative"),
            ("j k / Up Down", "select task / scroll narrative"),
            ("PgUp / PgDn", "page"),
            ("Home End / g G", "jump to start / end"),
            ("?", "toggle this help"),
        ],
        AttachHelpMode::Campaign => vec![
            ("q / Ctrl-D", "detach (campaign keeps going)"),
            ("b / Esc / Backspace", "back"),
            ("Enter", "drill into the selected sub-plan"),
            ("r", "refresh"),
            ("j k / Up Down / Tab", "select sub-plan"),
            ("PgUp / PgDn", "page"),
            ("Home End / g G", "jump to start / end"),
            ("?", "toggle this help"),
        ],
        AttachHelpMode::Chain => vec![
            ("q / Esc / Ctrl-D", "detach (chain keeps going)"),
            ("Enter", "show the selected step's run"),
            ("r", "redo the selected step"),
            ("e", "extend the chain with a new step"),
            ("p", "pause the chain"),
            ("k", "kill the chain (asks to confirm)"),
            ("Up / Down", "select step"),
            ("PgUp / PgDn", "scroll activity"),
            ("?", "toggle this help"),
        ],
    }
}

/// Draw the `?` help overlay: a centered popup over the current frame. Any
/// key closes it.
pub(crate) fn render_help_overlay(frame: &mut ratatui::Frame<'_>, mode: AttachHelpMode) {
    let lines = help_overlay_lines(mode);
    let key_width = lines.iter().map(|(key, _)| key.len()).max().unwrap_or(0);
    let body: Vec<Line<'static>> = lines
        .iter()
        .map(|(key, action)| {
            Line::from(vec![
                Span::styled(
                    format!(" {key:key_width$}  "),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw((*action).to_string()),
            ])
        })
        .collect();
    let width = (key_width + 44).min(frame.area().width.saturating_sub(2) as usize) as u16;
    let height = (body.len() as u16 + 2).min(frame.area().height.saturating_sub(2));
    let area = centered_rect(frame.area(), width.max(20), height.max(3));
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(body).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("{} — any key closes", mode.title())),
        ),
        area,
    );
}

fn centered_rect(outer: ratatui::layout::Rect, width: u16, height: u16) -> ratatui::layout::Rect {
    let width = width.min(outer.width);
    let height = height.min(outer.height);
    ratatui::layout::Rect {
        x: outer.x + (outer.width.saturating_sub(width)) / 2,
        y: outer.y + (outer.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

pub(crate) fn deadreckoning_status_line(
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

pub(crate) fn render_spend(
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

pub(crate) fn render_context(
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

pub(crate) fn render_acceptance(
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

pub(crate) fn render_live_files(
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

pub(crate) fn render_processes(
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

pub(crate) fn turn_timer(
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

/// The single selection cursor used by every attach surface (run focused panel,
/// plan task, campaign sub-goal, chain step). `>` marks the selection.
pub(crate) fn selection_glyph(selected: bool) -> &'static str {
    if selected { ">" } else { " " }
}

/// The single scroll-position readout shown on every list panel title:
/// " first-last/total" for the visible window, or "" when everything fits.
pub(crate) fn scroll_indicator(offset: usize, rows: usize, total: usize) -> String {
    if total <= rows || total == 0 {
        return String::new();
    }
    let first = offset.saturating_add(1).min(total);
    let last = offset.saturating_add(rows).min(total);
    if first == last {
        // A panes-based surface (rows = 1) gets a single selection readout.
        format!(" {first}/{total}")
    } else {
        format!(" {first}-{last}/{total}")
    }
}

/// The window start that keeps `selected` visible within `rows` of `total`
/// items (selection rides the bottom edge while scrolling down).
pub(crate) fn list_scroll_offset(selected: usize, rows: usize, total: usize) -> usize {
    if rows == 0 || total <= rows || selected < rows {
        0
    } else {
        (selected + 1)
            .saturating_sub(rows)
            .min(total.saturating_sub(rows))
    }
}

pub(crate) fn panel_title(
    title: &str,
    focused: bool,
    offset: usize,
    rows: usize,
    total: usize,
) -> String {
    format!(
        "{}{title}{}",
        selection_glyph(focused),
        scroll_indicator(offset, rows, total)
    )
}
