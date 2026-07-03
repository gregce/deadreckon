use super::super::*;
use super::panes::docs::render_markdown_doc_lines;
use super::panes::narrative::run_narrative_lines;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachPanel {
    Activity,
    Files,
    Processes,
}

impl AttachPanel {
    fn next(self) -> Self {
        match self {
            Self::Activity => Self::Files,
            Self::Files => Self::Processes,
            Self::Processes => Self::Activity,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Activity => Self::Processes,
            Self::Files => Self::Activity,
            Self::Processes => Self::Files,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AttachCampaignParent {
    pub(crate) campaign_id: String,
    pub(crate) sub_id: String,
}

#[derive(Debug, Clone)]
pub(crate) struct AttachParentPlan {
    pub(crate) plan_id: String,
    pub(crate) task_id: String,
    pub(crate) campaign_parent: Option<AttachCampaignParent>,
}

#[derive(Debug, Clone)]
pub(crate) struct AttachTuiState {
    pub(crate) focused_panel: AttachPanel,
    pub(crate) activity_scroll: usize,
    pub(crate) docs_scroll: usize,
    pub(crate) docs_open: bool,
    pub(crate) narrative_scroll: usize,
    pub(crate) files_scroll: usize,
    pub(crate) processes_scroll: usize,
    pub(crate) view: AttachViewMode,
    pub(crate) visual: NarrativeVisualMode,
    pub(crate) show_completion_actions: bool,
    pub(crate) post_action_notice: Option<AttachActionNotice>,
    pub(crate) narrative_notice: Option<String>,
    pub(crate) narrative_projection: Option<narrative::NarrativeProjection>,
    pub(crate) parent_plan: Option<AttachParentPlan>,
    pub(crate) pending_confirm: Option<CompletionAction>,
}

impl Default for AttachTuiState {
    fn default() -> Self {
        Self {
            focused_panel: AttachPanel::Activity,
            activity_scroll: 0,
            docs_scroll: 0,
            docs_open: false,
            narrative_scroll: 0,
            files_scroll: 0,
            processes_scroll: 0,
            view: AttachViewMode::Activity,
            visual: NarrativeVisualMode::Architecture,
            show_completion_actions: true,
            post_action_notice: None,
            narrative_notice: None,
            narrative_projection: None,
            parent_plan: None,
            pending_confirm: None,
        }
    }
}

impl AttachTuiState {
    pub(crate) fn handle_key(
        &mut self,
        key: KeyEvent,
        counts: AttachPanelCounts,
        rows: AttachPanelRows,
    ) {
        {
            let mut nav = RunNav {
                state: self,
                counts,
                rows,
            };
            crate::tui::navigation::dispatch_navigation(&mut nav, key);
        }
        self.clamp(counts, rows);
    }

    pub(crate) fn toggle_docs(&mut self) {
        self.docs_open = !self.docs_open;
        if self.docs_open {
            self.view = AttachViewMode::Activity;
        }
        self.focused_panel = AttachPanel::Activity;
        self.post_action_notice = None;
    }

    pub(crate) fn toggle_view(&mut self) {
        self.docs_open = false;
        self.view = toggle_attach_view(self.view);
        self.focused_panel = AttachPanel::Activity;
        self.post_action_notice = None;
        self.narrative_notice = None;
        if !self.view.is_narrative() {
            self.narrative_projection = None;
        }
    }

    pub(crate) fn cycle_visual(&mut self) {
        self.visual = self.visual.next();
        self.docs_open = false;
        if self.view == AttachViewMode::Activity {
            self.view = AttachViewMode::Narrative;
        }
        self.focused_panel = AttachPanel::Activity;
    }

    pub(crate) fn record_narrative_refresh(&mut self, notice: String) {
        self.docs_open = false;
        if self.view == AttachViewMode::Activity {
            self.view = AttachViewMode::Narrative;
        }
        self.narrative_notice = Some(notice);
        self.focused_panel = AttachPanel::Activity;
    }

    pub(crate) fn record_post_action(&mut self, notice: AttachActionNotice) {
        self.docs_open = false;
        self.view = AttachViewMode::Activity;
        self.focused_panel = AttachPanel::Activity;
        self.activity_scroll = 0;
        self.docs_scroll = 0;
        self.narrative_scroll = 0;
        self.files_scroll = 0;
        self.processes_scroll = 0;
        self.post_action_notice = Some(notice);
        self.narrative_notice = None;
        self.narrative_projection = None;
    }

    pub(crate) fn scroll_focused(
        &mut self,
        delta: isize,
        counts: AttachPanelCounts,
        rows: AttachPanelRows,
    ) {
        let current = self.focused_scroll();
        let max = max_panel_scroll(self.focused_panel, counts, rows);
        let next = if delta.is_negative() {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current.saturating_add(delta as usize)
        };
        self.set_focused_scroll(next.min(max));
    }

    pub(crate) fn clamp(&mut self, counts: AttachPanelCounts, rows: AttachPanelRows) {
        self.activity_scroll =
            self.activity_scroll
                .min(max_panel_scroll(AttachPanel::Activity, counts, rows));
        self.docs_scroll =
            self.docs_scroll
                .min(max_panel_scroll(AttachPanel::Activity, counts, rows));
        self.narrative_scroll =
            self.narrative_scroll
                .min(max_panel_scroll(AttachPanel::Activity, counts, rows));
        self.files_scroll =
            self.files_scroll
                .min(max_panel_scroll(AttachPanel::Files, counts, rows));
        self.processes_scroll =
            self.processes_scroll
                .min(max_panel_scroll(AttachPanel::Processes, counts, rows));
    }

    fn focused_scroll(&self) -> usize {
        match self.focused_panel {
            AttachPanel::Activity if self.docs_open => self.docs_scroll,
            AttachPanel::Activity if self.view.is_narrative() => self.narrative_scroll,
            AttachPanel::Activity => self.activity_scroll,
            AttachPanel::Files => self.files_scroll,
            AttachPanel::Processes => self.processes_scroll,
        }
    }

    fn set_focused_scroll(&mut self, offset: usize) {
        match self.focused_panel {
            AttachPanel::Activity if self.docs_open => self.docs_scroll = offset,
            AttachPanel::Activity if self.view.is_narrative() => self.narrative_scroll = offset,
            AttachPanel::Activity => self.activity_scroll = offset,
            AttachPanel::Files => self.files_scroll = offset,
            AttachPanel::Processes => self.processes_scroll = offset,
        }
    }
}

/// Drives the run panel's multi-panel scroll model through the shared
/// [`dispatch_navigation`](crate::tui::navigation::dispatch_navigation). The run
/// panel is the reference; these hooks reproduce its original `handle_key`
/// semantics exactly (same methods, same order).
struct RunNav<'a> {
    state: &'a mut AttachTuiState,
    counts: AttachPanelCounts,
    rows: AttachPanelRows,
}

impl crate::tui::navigation::NavigableSurface for RunNav<'_> {
    fn focus_next(&mut self) {
        self.state.focused_panel = self.state.focused_panel.next();
    }

    fn focus_previous(&mut self) {
        self.state.focused_panel = self.state.focused_panel.previous();
    }

    fn scroll_lines(&mut self, delta: isize) {
        self.state.scroll_focused(delta, self.counts, self.rows);
    }

    fn scroll_page(&mut self, direction: isize) {
        let delta = direction.signum() * page_delta(self.state.focused_panel, self.rows);
        self.state.scroll_focused(delta, self.counts, self.rows);
    }

    fn scroll_to_start(&mut self) {
        self.state.set_focused_scroll(0);
    }

    fn scroll_to_end(&mut self) {
        let max = max_panel_scroll(self.state.focused_panel, self.counts, self.rows);
        self.state.set_focused_scroll(max);
    }
}

pub(crate) fn toggle_attach_view(view: AttachViewMode) -> AttachViewMode {
    match view {
        AttachViewMode::Activity => AttachViewMode::Narrative,
        AttachViewMode::Narrative | AttachViewMode::Split => AttachViewMode::Activity,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AttachActionNotice {
    pub(crate) action: CompletionAction,
    pub(crate) success: bool,
}

impl AttachActionNotice {
    pub(crate) fn lines(&self) -> Vec<String> {
        let (kind, what, why, evidence, secondary) = if self.success {
            (
                VerdictKind::Completed,
                self.action.success_detail().to_string(),
                "The action completed inside attach, so detaching returns to the shell with the updated state available for inspection."
                    .to_string(),
                vec![
                    ("action".to_string(), self.action.label().to_string()),
                    ("result".to_string(), "succeeded".to_string()),
                ],
                vec![
                    ("Secondary", "deadreckon status"),
                    ("Secondary", "deadreckon list"),
                ],
            )
        } else {
            (
                VerdictKind::Failed,
                "The attach action failed before completing the requested state change."
                    .to_string(),
                "The terminal output above contains the concrete error; detaching is the safest next step before retrying after the error is fixed."
                    .to_string(),
                vec![
                    ("action".to_string(), self.action.label().to_string()),
                    ("result".to_string(), "failed".to_string()),
                ],
                vec![("Secondary", "retry the action after fixing the error")],
            )
        };
        VerdictSurface::must_new(
            kind,
            format!("{} action", self.action.label()),
            None,
            ExplanationPanel::new(what, why, evidence),
            vec![("Recommended", "q detach")],
            secondary,
        )
        .render_plain(false)
        .lines()
        .map(ToString::to_string)
        .collect()
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AttachPanelCounts {
    pub(crate) activity: usize,
    pub(crate) files: usize,
    pub(crate) processes: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AttachPanelRows {
    pub(crate) activity: usize,
    pub(crate) files: usize,
    pub(crate) processes: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AttachPanelLayout {
    pub(crate) header: ratatui::layout::Rect,
    pub(crate) activity: ratatui::layout::Rect,
    pub(crate) files: ratatui::layout::Rect,
    pub(crate) processes: ratatui::layout::Rect,
    pub(crate) footer: ratatui::layout::Rect,
    pub(crate) rows: AttachPanelRows,
}

impl AttachPanelLayout {
    pub(crate) fn panel_at(&self, column: u16, row: u16) -> Option<AttachPanel> {
        if rect_contains(self.activity, column, row) {
            Some(AttachPanel::Activity)
        } else if rect_contains(self.files, column, row) {
            Some(AttachPanel::Files)
        } else if rect_contains(self.processes, column, row) {
            Some(AttachPanel::Processes)
        } else {
            None
        }
    }
}

pub(crate) fn attach_panel_layout(area: ratatui::layout::Rect) -> AttachPanelLayout {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(10),
            Constraint::Length(4),
            Constraint::Length(2),
        ])
        .split(area);
    let center = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(60), Constraint::Length(44)])
        .split(vertical[1]);
    AttachPanelLayout {
        header: vertical[0],
        activity: center[0],
        files: center[1],
        processes: vertical[2],
        footer: vertical[3],
        rows: AttachPanelRows {
            activity: panel_inner_rows(center[0]),
            files: panel_inner_rows(center[1]),
            processes: panel_inner_rows(vertical[2]),
        },
    }
}

pub(crate) fn attach_panel_counts(
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    events: &[RunEvent],
    live: &AttachLive,
    tui_state: &AttachTuiState,
) -> AttachPanelCounts {
    AttachPanelCounts {
        activity: if tui_state.docs_open && state.status == RunStatus::Completed {
            render_markdown_doc_lines(state).len()
        } else if tui_state.view.is_narrative() {
            run_narrative_lines(state, spend, traces, events, live, tui_state).len()
        } else {
            attach_activity_lines_for_tui(state, spend, traces, events, live, tui_state).len()
        },
        files: live_file_lines(live).len(),
        processes: process_lines(live).len(),
    }
}

fn panel_inner_rows(area: ratatui::layout::Rect) -> usize {
    area.height.saturating_sub(2) as usize
}

fn rect_contains(rect: ratatui::layout::Rect, column: u16, row: u16) -> bool {
    column >= rect.x
        && column < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

fn page_delta(panel: AttachPanel, rows: AttachPanelRows) -> isize {
    let rows = panel_rows(panel, rows).max(1);
    rows.saturating_sub(1).max(1) as isize
}

pub(crate) fn max_panel_scroll(
    panel: AttachPanel,
    counts: AttachPanelCounts,
    rows: AttachPanelRows,
) -> usize {
    panel_count(panel, counts).saturating_sub(panel_rows(panel, rows))
}

fn panel_count(panel: AttachPanel, counts: AttachPanelCounts) -> usize {
    match panel {
        AttachPanel::Activity => counts.activity,
        AttachPanel::Files => counts.files,
        AttachPanel::Processes => counts.processes,
    }
}

fn panel_rows(panel: AttachPanel, rows: AttachPanelRows) -> usize {
    match panel {
        AttachPanel::Activity => rows.activity,
        AttachPanel::Files => rows.files,
        AttachPanel::Processes => rows.processes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    const COUNTS: AttachPanelCounts = AttachPanelCounts {
        activity: 100,
        files: 10,
        processes: 5,
    };
    const ROWS: AttachPanelRows = AttachPanelRows {
        activity: 20,
        files: 5,
        processes: 3,
    };

    #[test]
    fn run_panel_key_handling_unchanged_after_extraction() {
        let mut s = AttachTuiState::default();

        // Down / j scroll the focused (activity) panel; Up / k reverse.
        s.handle_key(key(KeyCode::Down), COUNTS, ROWS);
        assert_eq!(s.activity_scroll, 1);
        s.handle_key(key(KeyCode::Char('j')), COUNTS, ROWS);
        assert_eq!(s.activity_scroll, 2);
        s.handle_key(key(KeyCode::Up), COUNTS, ROWS);
        assert_eq!(s.activity_scroll, 1);

        // End / G jump to the bottom; Home / g back to the top.
        s.handle_key(key(KeyCode::End), COUNTS, ROWS);
        assert_eq!(
            s.activity_scroll,
            max_panel_scroll(AttachPanel::Activity, COUNTS, ROWS)
        );
        s.handle_key(key(KeyCode::Home), COUNTS, ROWS);
        assert_eq!(s.activity_scroll, 0);

        // PageDown advances by one page; PageUp returns.
        s.handle_key(key(KeyCode::PageDown), COUNTS, ROWS);
        assert_eq!(
            s.activity_scroll,
            page_delta(AttachPanel::Activity, ROWS) as usize
        );
        s.handle_key(key(KeyCode::PageUp), COUNTS, ROWS);
        assert_eq!(s.activity_scroll, 0);

        // Tab / BackTab cycle the focused panel.
        assert_eq!(s.focused_panel, AttachPanel::Activity);
        s.handle_key(key(KeyCode::Tab), COUNTS, ROWS);
        assert_eq!(s.focused_panel, AttachPanel::Files);
        s.handle_key(key(KeyCode::BackTab), COUNTS, ROWS);
        assert_eq!(s.focused_panel, AttachPanel::Activity);

        // A key the shared core does not own is a no-op.
        s.handle_key(key(KeyCode::Char('z')), COUNTS, ROWS);
        assert_eq!(s.activity_scroll, 0);
        assert_eq!(s.focused_panel, AttachPanel::Activity);
    }
}
