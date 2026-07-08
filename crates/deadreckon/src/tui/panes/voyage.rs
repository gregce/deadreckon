use std::io;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use tui_tree_widget::{Tree, TreeItem, TreeState};
use unicode_width::UnicodeWidthChar;

use crate::tui::tree::{NodeId, NodeKind, NodeStatus, TreeModel, TreeNode};
use crate::{run_prefix, ui};

#[derive(Debug, Default)]
pub(crate) struct VoyagePaneState {
    tree: TreeState<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct VoyageZoomState {
    zoomed: Option<NodeId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VoyageZoomAction {
    Zoomed,
    BackedOut,
    Quit,
}

impl VoyageZoomState {
    pub(crate) fn enter(&mut self, selected: NodeId) -> VoyageZoomAction {
        self.zoomed = Some(selected);
        VoyageZoomAction::Zoomed
    }

    pub(crate) fn escape(&mut self) -> VoyageZoomAction {
        if self.zoomed.take().is_some() {
            VoyageZoomAction::BackedOut
        } else {
            VoyageZoomAction::Quit
        }
    }

    pub(crate) fn zoomed(&self) -> Option<&NodeId> {
        self.zoomed.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn breadcrumb(&self, model: &TreeModel) -> String {
        self.zoomed
            .as_ref()
            .and_then(|id| node_breadcrumb(model, id))
            .unwrap_or_else(|| "voyage".to_string())
    }
}

/// Enter on a node that is already zoomed promotes: when the node is backed by
/// a child run, the attach session leaves the in-frame detail pane and opens
/// the full run surface for it. First Enter (or Enter on a task with no child
/// run yet) stays an in-frame zoom.
pub(crate) fn zoomed_run_to_promote(
    zoom: &VoyageZoomState,
    target: &NodeId,
    child_run_id: Option<&str>,
) -> Option<String> {
    let run_id = child_run_id?;
    (zoom.zoomed() == Some(target)).then(|| run_id.to_string())
}

impl VoyagePaneState {
    #[cfg(test)]
    pub(crate) fn selected_path(&self) -> &[String] {
        self.tree.selected()
    }

    pub(crate) fn select_node(&mut self, model: &TreeModel, id: &NodeId) -> bool {
        let Some(path) = path_to_node(&model.root, id, &mut Vec::new()) else {
            return false;
        };
        self.open_all(model);
        self.tree.select(path);
        self.tree.scroll_selected_into_view();
        true
    }

    pub(crate) fn sync(&mut self, model: &TreeModel) {
        self.open_all(model);
        let selected = self.tree.selected().to_vec();
        if selected.is_empty() || !path_exists(&model.root, &selected, 0) {
            self.tree.select(vec![node_segment(&model.root.id)]);
        }
        self.tree.scroll_selected_into_view();
    }

    fn open_all(&mut self, model: &TreeModel) {
        open_node_paths(&mut self.tree, &model.root, &mut Vec::new());
    }
}

pub(crate) fn render_voyage_pane(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    model: &TreeModel,
    state: &mut VoyagePaneState,
) {
    state.sync(model);
    let width = usize::from(area.width.saturating_sub(4)).max(1);
    let items = match tree_items(model, width) {
        Ok(items) => items,
        Err(err) => {
            frame.render_widget(
                Paragraph::new(format!("voyage unavailable: {err}"))
                    .block(Block::default().title("voyage").borders(Borders::ALL)),
                area,
            );
            return;
        }
    };
    let tree = match Tree::new(&items) {
        Ok(tree) => tree,
        Err(err) => {
            frame.render_widget(
                Paragraph::new(format!("voyage unavailable: {err}"))
                    .block(Block::default().title("voyage").borders(Borders::ALL)),
                area,
            );
            return;
        }
    }
    .block(Block::default().title("voyage").borders(Borders::ALL))
    .highlight_symbol("> ")
    .highlight_style(
        Style::default()
            .fg(ui::TUI_PALETTE.border_focused)
            .add_modifier(Modifier::BOLD),
    )
    .node_open_symbol("▾ ")
    .node_closed_symbol("▸ ")
    .node_no_children_symbol("  ");
    frame.render_stateful_widget(tree, area, &mut state.tree);
}

pub(crate) fn collapsed_voyage_header(model: &TreeModel, width: usize) -> String {
    let row = node_row(&model.root, width.saturating_sub("voyage ".len()), 0);
    format!("voyage {row}")
}

pub(crate) fn node_breadcrumb(model: &TreeModel, id: &NodeId) -> Option<String> {
    breadcrumb_to_node(&model.root, id, &mut Vec::new()).map(|parts| parts.join(" > "))
}

fn tree_items(model: &TreeModel, width: usize) -> io::Result<Vec<TreeItem<'static, String>>> {
    Ok(vec![tree_item(&model.root, width, 0)?])
}

fn tree_item(node: &TreeNode, width: usize, depth: usize) -> io::Result<TreeItem<'static, String>> {
    let id = node_segment(&node.id);
    let row = node_row(node, width, depth);
    let children = node
        .children
        .iter()
        .map(|child| tree_item(child, width, depth + 1))
        .collect::<io::Result<Vec<_>>>()?;
    if children.is_empty() {
        Ok(TreeItem::new_leaf(id, row))
    } else {
        TreeItem::new(id, row, children)
    }
}

fn node_row(node: &TreeNode, width: usize, depth: usize) -> String {
    let suffix = node_suffix(node);
    let prefix = format!(
        "{} {} {} ",
        status_glyph(node.status),
        kind_label(node.kind),
        node_ref(&node.id)
    );
    let reserved = visible_width(&prefix)
        .saturating_add(visible_width(&suffix))
        .saturating_add(depth.saturating_mul(2));
    let label_width = width.saturating_sub(reserved).max(8);
    let row = format!(
        "{prefix}{}{}",
        truncate_visible(&node.label, label_width),
        suffix
    );
    truncate_visible(&row, width)
}

fn node_suffix(node: &TreeNode) -> String {
    let mut parts = Vec::new();
    if let Some((passed, total)) = node.gate {
        parts.push(format!("{passed}/{total}"));
    }
    if let Some(spend) = node.spend {
        parts.push(format!("${spend:.2}"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("  {}", parts.join("  "))
    }
}

fn kind_label(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Run => "run",
        NodeKind::Plan => "plan",
        NodeKind::Task => "task",
        NodeKind::Chain => "chain",
        NodeKind::ChainStep => "step",
        NodeKind::Campaign => "campaign",
        NodeKind::SubGoal => "sub",
    }
}

fn node_ref(id: &NodeId) -> String {
    match id {
        NodeId::Run(id) | NodeId::Plan(id) | NodeId::Chain(id) | NodeId::Campaign(id) => {
            run_prefix(id)
        }
        NodeId::Task { task_id, .. }
        | NodeId::SubGoal {
            sub_id: task_id, ..
        } => task_id.clone(),
        NodeId::ChainStep { step_index, .. } => format!("{}", step_index + 1),
    }
}

fn status_glyph(status: NodeStatus) -> &'static str {
    match status {
        NodeStatus::Pending => "○",
        NodeStatus::Running | NodeStatus::Gated => "●",
        NodeStatus::Verified => "✓",
        NodeStatus::Failed | NodeStatus::Killed => "✗",
        NodeStatus::Paused => "⏸",
    }
}

fn node_segment(id: &NodeId) -> String {
    match id {
        NodeId::Run(id) => format!("run:{id}"),
        NodeId::Plan(id) => format!("plan:{id}"),
        NodeId::Task { plan_id, task_id } => format!("task:{plan_id}:{task_id}"),
        NodeId::Chain(id) => format!("chain:{id}"),
        NodeId::ChainStep {
            chain_id,
            step_index,
        } => {
            format!("step:{chain_id}:{step_index}")
        }
        NodeId::Campaign(id) => format!("campaign:{id}"),
        NodeId::SubGoal {
            campaign_id,
            sub_id,
        } => {
            format!("sub:{campaign_id}:{sub_id}")
        }
    }
}

fn path_to_node(node: &TreeNode, id: &NodeId, path: &mut Vec<String>) -> Option<Vec<String>> {
    path.push(node_segment(&node.id));
    if &node.id == id {
        let found = path.clone();
        path.pop();
        return Some(found);
    }
    for child in &node.children {
        if let Some(found) = path_to_node(child, id, path) {
            path.pop();
            return Some(found);
        }
    }
    path.pop();
    None
}

fn breadcrumb_to_node(
    node: &TreeNode,
    id: &NodeId,
    parts: &mut Vec<String>,
) -> Option<Vec<String>> {
    parts.push(format!("{} {}", kind_label(node.kind), node_ref(&node.id)));
    if &node.id == id {
        let found = parts.clone();
        parts.pop();
        return Some(found);
    }
    for child in &node.children {
        if let Some(found) = breadcrumb_to_node(child, id, parts) {
            parts.pop();
            return Some(found);
        }
    }
    parts.pop();
    None
}

fn path_exists(node: &TreeNode, path: &[String], depth: usize) -> bool {
    let Some(segment) = path.get(depth) else {
        return false;
    };
    if *segment != node_segment(&node.id) {
        return false;
    }
    if depth + 1 == path.len() {
        return true;
    }
    node.children
        .iter()
        .any(|child| path_exists(child, path, depth + 1))
}

fn open_node_paths(state: &mut TreeState<String>, node: &TreeNode, path: &mut Vec<String>) {
    path.push(node_segment(&node.id));
    if !node.children.is_empty() {
        state.open(path.clone());
        for child in &node.children {
            open_node_paths(state, child, path);
        }
    }
    path.pop();
}

fn truncate_visible(text: &str, max_width: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if visible_width(&text) <= max_width {
        return text;
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }
    let mut out = String::new();
    let mut width = 0usize;
    let limit = max_width - 3;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > limit {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push_str("...");
    out
}

fn visible_width(text: &str) -> usize {
    ui::display_width(text)
}
