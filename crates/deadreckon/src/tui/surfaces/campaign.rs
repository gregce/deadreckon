use crate::commands::campaign::{
    CampaignAttachState, CampaignFeedEvent, campaign_status_text, rollup_verdict_text,
};
use crate::tui::panes::activity::{scroll_indicator, selection_glyph};
use crate::tui::panes::detail::run_detail_lines;
use crate::tui::panes::footer::footer;
use crate::tui::panes::voyage::{VoyagePaneState, node_breadcrumb, render_voyage_pane};
use crate::tui::spine::{render_spine_band, spine_for_campaign_with_events};
use crate::tui::surfaces::plan::{plan_event_line, plan_task_detail_lines};
use crate::tui::tree::{NodeId, tree_for_campaign};
use crate::{load_plan, load_run, one_line, plan_status_label, run_prefix, ui};
use chrono::Utc;
use deadreckon_core::campaign::SubGoalStatus;
use deadreckon_core::{DeadreckonPaths, Plan};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

pub(crate) fn render_campaign_attach(frame: &mut ratatui::Frame<'_>, state: &CampaignAttachState) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Min(8),
            Constraint::Length(7),
            Constraint::Length(4),
            Constraint::Length(2),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(campaign_attach_header_lines(state))
            .block(
                Block::default()
                    .title(format!(
                        "deadreckon campaign{}",
                        scroll_indicator(state.selected, 1, state.campaign.sub_goals.len())
                    ))
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: true }),
        vertical[0],
    );

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(42), Constraint::Min(48)])
        .split(vertical[1]);
    let tree = tree_for_campaign(&state.paths, &state.campaign);
    let mut voyage_state = VoyagePaneState::default();
    let effective_node = state.zoomed_node.as_ref().or(state.selected_node.as_ref());
    if let Some(node) = effective_node {
        voyage_state.select_node(&tree, node);
    } else if let Some(sub) = state.campaign.sub_goals.get(state.selected) {
        voyage_state.select_node(
            &tree,
            &NodeId::sub_goal(&state.campaign.campaign_id, &sub.sub_id),
        );
    } else {
        voyage_state.sync(&tree);
    }
    render_voyage_pane(frame, body[0], &tree, &mut voyage_state);

    let rendered_detail = if let Some(node) = effective_node {
        tree.find(node).is_some()
            && render_campaign_node_detail(
                frame,
                body[1],
                state,
                &tree,
                node,
                state.zoomed_node.is_some(),
            )
    } else {
        false
    };
    if !rendered_detail {
        render_campaign_sub_cards(frame, body[1], state);
    }

    let feed = campaign_feed_lines(state, vertical[2].height.saturating_sub(2) as usize)
        .into_iter()
        .map(ListItem::new)
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(feed).block(
            Block::default()
                .title("campaign feed")
                .borders(Borders::ALL),
        ),
        vertical[2],
    );
    let campaign_events = state
        .feed
        .iter()
        .filter_map(|event| match event {
            CampaignFeedEvent::Campaign { event } => Some(event.clone()),
            CampaignFeedEvent::SubPlan { .. }
            | CampaignFeedEvent::Snapshot { .. }
            | CampaignFeedEvent::Warning { .. } => None,
        })
        .collect::<Vec<_>>();
    let spine = spine_for_campaign_with_events(&state.campaign, &campaign_events, Utc::now());
    render_spine_band(frame, vertical[3], &spine);
    frame.render_widget(
        Paragraph::new(campaign_attach_footer_text(false)),
        vertical[4],
    );
}

#[cfg(test)]
pub(crate) fn render_campaign_attach_text(state: &CampaignAttachState, plain: bool) -> String {
    let mut lines = Vec::new();
    lines.push("deadreckon campaign".to_string());
    lines.extend(campaign_attach_header_text_lines(state));
    lines.push("sub-plans".to_string());
    for index in 0..state.campaign.sub_goals.len() {
        lines.extend(campaign_sub_lines(state, index, 120));
    }
    lines.push("campaign feed".to_string());
    lines.extend(campaign_feed_text_lines(state, 12));
    lines.push("status spine".to_string());
    let campaign_events = state
        .feed
        .iter()
        .filter_map(|event| match event {
            CampaignFeedEvent::Campaign { event } => Some(event.clone()),
            CampaignFeedEvent::SubPlan { .. }
            | CampaignFeedEvent::Snapshot { .. }
            | CampaignFeedEvent::Warning { .. } => None,
        })
        .collect::<Vec<_>>();
    lines.extend(crate::tui::spine::spine_plain_lines(
        &spine_for_campaign_with_events(&state.campaign, &campaign_events, Utc::now()),
    ));
    lines.push(campaign_attach_footer_text(plain));
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn campaign_attach_header_lines(state: &CampaignAttachState) -> Vec<Line<'static>> {
    campaign_attach_header_text_lines(state)
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                let split = line.find("  ").unwrap_or(line.len());
                let (prefix, rest) = line.split_at(split);
                Line::from(vec![
                    Span::styled(prefix.to_string(), Style::default().fg(Color::Cyan)),
                    Span::raw(rest.to_string()),
                ])
            } else {
                Line::from(line)
            }
        })
        .collect()
}

fn campaign_attach_header_text_lines(state: &CampaignAttachState) -> Vec<String> {
    let rollup = state
        .rollup
        .as_ref()
        .map(|rollup| rollup_verdict_text(rollup.rollup_verdict))
        .unwrap_or("-");
    let (merged, running, pending) = campaign_sub_counts(state);
    vec![
        format!(
            "campaign {}  status {}  roll-up {}  subs {}/{}/{}",
            run_prefix(&state.campaign.campaign_id),
            campaign_status_text(state.campaign.status),
            rollup,
            merged,
            running,
            pending
        ),
        one_line(&state.campaign.root_goal, 120),
        format!(
            "tree budget {} / {}",
            money_label(Some(state.aggregate_spend_usd)),
            money_label(state.campaign.tree_budget_usd)
        ),
        format!(
            "breadcrumb campaign {}",
            run_prefix(&state.campaign.campaign_id)
        ),
    ]
}

fn campaign_sub_counts(state: &CampaignAttachState) -> (usize, usize, usize) {
    let merged = state
        .campaign
        .sub_goals
        .iter()
        .filter(|sub| sub.status == SubGoalStatus::Merged)
        .count();
    let running = state
        .campaign
        .sub_goals
        .iter()
        .filter(|sub| sub.status == SubGoalStatus::Running)
        .count();
    let pending = state
        .campaign
        .sub_goals
        .len()
        .saturating_sub(merged + running);
    (merged, running, pending)
}

fn campaign_sub_lines(state: &CampaignAttachState, index: usize, width: usize) -> Vec<String> {
    let Some(sub) = state.campaign.sub_goals.get(index) else {
        return Vec::new();
    };
    let marker = selection_glyph(index == state.selected);
    let plan = sub
        .sub_plan_id
        .as_deref()
        .map(run_prefix)
        .unwrap_or_else(|| "-".to_string());
    let result = sub
        .result_run_id
        .as_deref()
        .map(run_prefix)
        .unwrap_or_else(|| "-".to_string());
    let spend = state.sub_spend_usd.get(&sub.sub_id).copied().unwrap_or(0.0);
    vec![
        one_line(
            &format!(
                "{marker} {} {}  plan={}  result={}  spend {}",
                sub.sub_id,
                campaign_sub_status_text(sub.status),
                plan,
                result,
                money_label(Some(spend))
            ),
            width,
        ),
        one_line(&format!("  {}", sub.goal), width),
    ]
}

fn render_campaign_sub_cards(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &CampaignAttachState,
) {
    let panes = campaign_sub_pane_layout(area, state.campaign.sub_goals.len());
    for (index, sub) in state.campaign.sub_goals.iter().enumerate() {
        let Some(rect) = panes.get(index).copied() else {
            continue;
        };
        let selected = index == state.selected;
        let title = format!(
            "{} {} {}",
            selection_glyph(selected),
            sub.sub_id,
            campaign_sub_status_text(sub.status)
        );
        let lines = campaign_sub_lines(state, index, rect.width.saturating_sub(4) as usize)
            .into_iter()
            .map(Line::from)
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(title)
                        .borders(Borders::ALL)
                        .border_style(if selected {
                            Style::default().fg(ui::TUI_PALETTE.border_focused)
                        } else {
                            Style::default().fg(ui::TUI_PALETTE.border_idle)
                        }),
                )
                .wrap(Wrap { trim: true }),
            rect,
        );
    }
}

fn render_campaign_node_detail(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &CampaignAttachState,
    tree: &crate::tui::tree::TreeModel,
    node: &NodeId,
    zoomed: bool,
) -> bool {
    let width = area.width.saturating_sub(4) as usize;
    let mut lines = match node {
        NodeId::Run(run_id) => load_run(&state.paths, run_id)
            .map(|run| run_detail_lines(&run, width))
            .unwrap_or_else(|_| vec![format!("run {} unavailable", run_prefix(run_id))]),
        NodeId::Task { plan_id, task_id } => {
            let Ok(plan) = load_plan(&state.paths, plan_id) else {
                return false;
            };
            let Some(task) = plan.tasks.iter().find(|task| task.task_id == *task_id) else {
                return false;
            };
            plan_task_detail_lines(&state.paths, &plan, task, width)
        }
        NodeId::Plan(plan_id) => {
            let Ok(plan) = load_plan(&state.paths, plan_id) else {
                return false;
            };
            plan_detail_lines(&state.paths, &plan, width)
        }
        NodeId::SubGoal {
            campaign_id,
            sub_id,
        } if campaign_id == &state.campaign.campaign_id => {
            let Some(index) = state
                .campaign
                .sub_goals
                .iter()
                .position(|sub| sub.sub_id == *sub_id)
            else {
                return false;
            };
            campaign_sub_lines(state, index, width)
        }
        NodeId::Campaign(campaign_id) if campaign_id == &state.campaign.campaign_id => {
            campaign_attach_header_text_lines(state)
        }
        _ => return false,
    };
    if zoomed && let Some(breadcrumb) = node_breadcrumb(tree, node) {
        lines.insert(0, format!("breadcrumb {breadcrumb}"));
    }
    frame.render_widget(
        Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>())
            .block(
                Block::default()
                    .title(campaign_detail_title(node, zoomed))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(ui::TUI_PALETTE.border_focused)),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
    true
}

fn plan_detail_lines(_paths: &DeadreckonPaths, plan: &Plan, width: usize) -> Vec<String> {
    vec![
        format!(
            "plan {}  status {}",
            run_prefix(&plan.plan_id),
            plan_status_label(plan.status)
        ),
        one_line(&plan.root_goal, width),
    ]
}

fn campaign_detail_title(node: &NodeId, zoomed: bool) -> String {
    let prefix = if zoomed { "zoom" } else { "detail" };
    match node {
        NodeId::Run(run_id) => format!("{prefix}: run {}", run_prefix(run_id)),
        NodeId::Task { task_id, .. } => format!("{prefix}: task {task_id}"),
        NodeId::Plan(plan_id) => format!("{prefix}: plan {}", run_prefix(plan_id)),
        NodeId::SubGoal { sub_id, .. } => format!("{prefix}: sub {sub_id}"),
        NodeId::Campaign(campaign_id) => format!("{prefix}: campaign {}", run_prefix(campaign_id)),
        NodeId::Chain(_) | NodeId::ChainStep { .. } => format!("{prefix}: voyage"),
    }
}

fn campaign_feed_lines(state: &CampaignAttachState, max: usize) -> Vec<Line<'static>> {
    campaign_feed_text_lines(state, max)
        .into_iter()
        .map(Line::from)
        .collect()
}

/// Friendly empty-state for the campaign feed: a next step, and never an
/// internal filename (it used to leak campaign-events.jsonl / plan-events.jsonl).
pub(crate) const CAMPAIGN_EMPTY_HINT: &str =
    "no campaign activity yet — sub-plans report progress as they run";

fn campaign_feed_text_lines(state: &CampaignAttachState, max: usize) -> Vec<String> {
    let mut lines = state
        .feed
        .iter()
        .rev()
        .take(max.max(1))
        .map(campaign_feed_event_line)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        lines.push(CAMPAIGN_EMPTY_HINT.to_string());
    }
    lines
}

fn campaign_feed_event_line(event: &CampaignFeedEvent) -> String {
    match event {
        CampaignFeedEvent::Campaign { event } => {
            let detail = if event.detail.is_null() {
                String::new()
            } else {
                format!(" {}", one_line(&event.detail.to_string(), 88))
            };
            format!(
                "{} campaign {}{}",
                event.ts.format("%H:%M:%S"),
                event.kind,
                detail
            )
        }
        CampaignFeedEvent::SubPlan { sub_id, event } => {
            format!("{sub_id}: {}", plan_event_line(event))
        }
        CampaignFeedEvent::Snapshot { campaign } => format!(
            "snapshot campaign {} {}",
            run_prefix(&campaign.campaign_id),
            campaign_status_text(campaign.status)
        ),
        CampaignFeedEvent::Warning { message } => {
            format!("warning {}", one_line(message, 96))
        }
    }
}

fn campaign_attach_footer_text(plain: bool) -> String {
    if plain {
        "plain summary: deadreckon attach <sub-plan-id> drills in".to_string()
    } else {
        footer(&[
            ("q/Esc/Ctrl-D", "detach"),
            ("b/Backspace", "back"),
            ("arrows/Tab", "select sub-plan"),
            ("Enter", "sub-plan"),
            ("r", "refresh"),
            ("?", "help"),
        ])
    }
}

fn campaign_sub_pane_layout(
    area: ratatui::layout::Rect,
    sub_count: usize,
) -> Vec<ratatui::layout::Rect> {
    if sub_count == 0 {
        return Vec::new();
    }
    let rows = if sub_count <= 3 { 1 } else { 2 };
    let row_constraints =
        std::iter::repeat_n(Constraint::Ratio(1, rows as u32), rows).collect::<Vec<_>>();
    let row_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(area);
    let mut rects = Vec::new();
    for row in 0..rows {
        let remaining = sub_count.saturating_sub(rects.len());
        let columns = remaining.min(if rows == 1 { sub_count } else { 3 }).max(1);
        let col_constraints =
            std::iter::repeat_n(Constraint::Ratio(1, columns as u32), columns).collect::<Vec<_>>();
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(col_constraints)
            .split(row_chunks[row]);
        for col in cols.iter().copied().take(remaining) {
            rects.push(col);
            if rects.len() == sub_count {
                break;
            }
        }
    }
    rects
}

fn campaign_sub_status_text(status: SubGoalStatus) -> &'static str {
    match status {
        SubGoalStatus::Pending => "pending",
        SubGoalStatus::Running => "running",
        SubGoalStatus::Merged => "merged",
        SubGoalStatus::Failed => "failed",
        SubGoalStatus::Killed => "killed",
    }
}

fn money_label(value: Option<f64>) -> String {
    value
        .map(|value| format!("${value:.2}"))
        .unwrap_or_else(|| "unbounded".to_string())
}
