use std::time::Duration;

use crate::commands::attach_runtime::AttachTickBudget;
use crate::commands::chain::{
    apply_mode_label, apply_strategy_label, branch_policy_label, chain_apply_strategy,
    chain_attach_summary_line, chain_step_dot, chain_step_status_label, on_fail_label, short_sha,
};
use crate::tui::panes::activity::{list_scroll_offset, scroll_indicator};
use crate::tui::panes::footer::footer;
use crate::tui::spine::{render_spine_band, spine_for_chain_with_events};
use chrono::Utc;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use deadreckon::verdict_surface::{ExplanationPanel, VerdictKind, VerdictSurface};
use deadreckon_core::{Chain, ChainEvent, ChainEventKind, ChainStatus};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use tui_textarea::{Input, Key, TextArea};

use crate::{CliError, one_line, run_prefix};

#[derive(Debug, Default)]
pub(crate) struct ChainAttachTuiState {
    pub(crate) selected_step: usize,
    pub(crate) events_scroll: u16,
    pub(crate) event_status_hint: Option<String>,
    pub(crate) modal: Option<AttachModal>,
}

#[derive(Debug, Clone)]
pub(crate) struct AttachModal {
    title: String,
    body: String,
    kind: AttachModalKind,
}

impl AttachModal {
    fn confirm_kill() -> Self {
        Self {
            title: "kill chain?".to_string(),
            body: "y confirm    n/Esc cancel".to_string(),
            kind: AttachModalKind::Confirm(ConfirmAction::Kill),
        }
    }

    fn line_input(
        title: impl Into<String>,
        body: impl Into<String>,
        placeholder: impl Into<String>,
    ) -> Self {
        let mut textarea = TextArea::default();
        textarea.set_placeholder_text(placeholder);
        textarea.set_cursor_line_style(Style::default());
        textarea.set_cursor_style(Style::default().fg(Color::Cyan));
        Self {
            title: title.into(),
            body: body.into(),
            kind: AttachModalKind::LineInput {
                textarea: Box::new(textarea),
            },
        }
    }

    fn notice(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            kind: AttachModalKind::Notice,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalKeyOutcome {
        match &mut self.kind {
            AttachModalKind::Confirm(action) => match key.code {
                KeyCode::Char('y') if key.modifiers.is_empty() => {
                    ModalKeyOutcome::Action(match action {
                        ConfirmAction::Kill => ChainModalAction::KillConfirmed,
                    })
                }
                KeyCode::Esc | KeyCode::Char('n') => ModalKeyOutcome::Close,
                _ => ModalKeyOutcome::Keep,
            },
            AttachModalKind::LineInput { textarea } => match key.code {
                KeyCode::Enter => ModalKeyOutcome::Action(ChainModalAction::ExtendSubmitted(
                    textarea.lines().first().cloned().unwrap_or_default(),
                )),
                KeyCode::Esc => ModalKeyOutcome::Close,
                _ => {
                    let _ = textarea.input(textarea_input(key));
                    ModalKeyOutcome::Keep
                }
            },
            AttachModalKind::Notice => match key.code {
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => ModalKeyOutcome::Close,
                _ => ModalKeyOutcome::Keep,
            },
        }
    }
}

#[derive(Debug, Clone)]
enum AttachModalKind {
    Confirm(ConfirmAction),
    LineInput { textarea: Box<TextArea<'static>> },
    Notice,
}

#[derive(Debug, Clone, Copy)]
enum ConfirmAction {
    Kill,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChainModalAction {
    None,
    KillConfirmed,
    ExtendSubmitted(String),
}

enum ModalKeyOutcome {
    Keep,
    Close,
    Action(ChainModalAction),
}

impl ChainAttachTuiState {
    pub(crate) fn clamp(&mut self, chain: &Chain) {
        if chain.steps.is_empty() {
            self.selected_step = 0;
            self.events_scroll = 0;
            return;
        }
        self.selected_step = self.selected_step.min(chain.steps.len() - 1);
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent, chain: &Chain) {
        {
            let mut nav = ChainNav { state: self, chain };
            crate::tui::navigation::dispatch_navigation(&mut nav, key);
        }
        self.clamp(chain);
    }

    pub(crate) fn open_kill_confirm(&mut self) {
        self.modal = Some(AttachModal::confirm_kill());
    }

    pub(crate) fn open_extend_input(&mut self) {
        self.modal = Some(AttachModal::line_input(
            "new chain step",
            "Enter submit    Esc cancel",
            "step goal",
        ));
    }

    pub(crate) fn open_notice(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.modal = Some(AttachModal::notice(title, body));
    }

    pub(crate) fn handle_key_with_modal(
        &mut self,
        key: KeyEvent,
        chain: &Chain,
    ) -> ChainModalAction {
        if let Some(mut modal) = self.modal.take() {
            return match modal.handle_key(key) {
                ModalKeyOutcome::Keep => {
                    self.modal = Some(modal);
                    ChainModalAction::None
                }
                ModalKeyOutcome::Close => ChainModalAction::None,
                ModalKeyOutcome::Action(action) => action,
            };
        }
        if key.modifiers == KeyModifiers::NONE {
            match key.code {
                KeyCode::Char('k') => {
                    self.open_kill_confirm();
                    return ChainModalAction::None;
                }
                KeyCode::Char('e') => {
                    self.open_extend_input();
                    return ChainModalAction::None;
                }
                _ => {}
            }
        }
        self.handle_key(key, chain);
        ChainModalAction::None
    }

    pub(crate) fn scroll(&mut self, delta: isize, chain: &Chain) {
        if chain.steps.is_empty() {
            return;
        }
        let next = (self.selected_step as isize + delta)
            .clamp(0, chain.steps.len().saturating_sub(1) as isize);
        self.selected_step = next as usize;
    }
}

/// Drives the chain attach step graph + events panel through the shared
/// navigation core. Arrows/`jk`/`Tab` move the selected step; `PgUp`/`PgDn`
/// scroll the events panel; `Home`/`End`/`g`/`G` jump to the first/last step
/// (Home also resets the events scroll).
struct ChainNav<'a> {
    state: &'a mut ChainAttachTuiState,
    chain: &'a Chain,
}

impl crate::tui::navigation::NavigableSurface for ChainNav<'_> {
    fn focus_next(&mut self) {
        self.state.scroll(1, self.chain);
    }

    fn focus_previous(&mut self) {
        self.state.scroll(-1, self.chain);
    }

    fn scroll_lines(&mut self, delta: isize) {
        self.state.scroll(delta, self.chain);
    }

    fn scroll_page(&mut self, direction: isize) {
        self.state.events_scroll = if direction < 0 {
            self.state.events_scroll.saturating_sub(8)
        } else {
            self.state.events_scroll.saturating_add(8)
        };
    }

    fn scroll_to_start(&mut self) {
        self.state.selected_step = 0;
        self.state.events_scroll = 0;
    }

    fn scroll_to_end(&mut self) {
        self.state.selected_step = self.chain.steps.len().saturating_sub(1);
    }
}

pub(crate) fn chain_event_read_hint(
    event_count: usize,
    appended_rows: usize,
    partial_bytes: usize,
    elapsed: Duration,
    budget: AttachTickBudget,
    error: Option<&CliError>,
) -> Option<String> {
    if let Some(error) = error {
        return Some(format!(
            "activity read delayed: {}",
            one_line(&error.to_string(), 72)
        ));
    }
    if elapsed > Duration::from_millis(budget.max_sync_io_ms) {
        return Some(format!(
            "activity catch-up: {event_count} events, +{appended_rows}, {}ms",
            elapsed.as_millis()
        ));
    }
    if partial_bytes > 0 {
        return Some(format!(
            "activity waiting for complete event line ({partial_bytes} bytes)"
        ));
    }
    None
}

fn chain_activity_title(tui_state: &ChainAttachTuiState) -> String {
    tui_state
        .event_status_hint
        .as_deref()
        .map(|hint| format!("chain activity - {}", one_line(hint, 72)))
        .unwrap_or_else(|| "chain activity".to_string())
}

pub(crate) fn render_chain_attach(
    frame: &mut ratatui::Frame<'_>,
    chain: &Chain,
    events: &[ChainEvent],
    tui_state: &ChainAttachTuiState,
) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(8),
            Constraint::Length(4),
            Constraint::Length(1),
        ])
        .split(area);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(rows[1]);

    frame.render_widget(
        Paragraph::new(chain_attach_header_text(chain)).block(
            Block::default()
                .borders(Borders::ALL)
                .title("deadreckon chain"),
        ),
        rows[0],
    );
    let timeline_lines = chain_timeline_lines(chain, tui_state);
    let steps_total = timeline_lines.len();
    let steps_rows = body[0].height.saturating_sub(2) as usize;
    let steps_offset = list_scroll_offset(tui_state.selected_step, steps_rows, steps_total);
    let timeline = timeline_lines
        .into_iter()
        .skip(steps_offset)
        .take(steps_rows.max(1))
        .map(ListItem::new)
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(timeline).block(Block::default().borders(Borders::ALL).title(format!(
            "steps{}",
            scroll_indicator(steps_offset, steps_rows, steps_total)
        ))),
        body[0],
    );
    let event_lines = chain_activity_lines(events, tui_state)
        .into_iter()
        .map(ListItem::new)
        .collect::<Vec<_>>();
    let activity_title = chain_activity_title(tui_state);
    frame.render_widget(
        List::new(event_lines).block(Block::default().borders(Borders::ALL).title(activity_title)),
        body[1],
    );
    let spine = spine_for_chain_with_events(chain, events, Utc::now());
    render_spine_band(frame, rows[2], &spine);
    frame.render_widget(Paragraph::new(chain_attach_footer_text(chain)), rows[3]);
    if let Some(modal) = &tui_state.modal {
        render_chain_modal(frame, modal);
    }
}

fn render_chain_modal(frame: &mut ratatui::Frame<'_>, modal: &AttachModal) {
    let area = centered_rect(frame.area(), 52, modal_height(modal));
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(modal.title.as_str())
        .borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    match &modal.kind {
        AttachModalKind::Confirm(_) | AttachModalKind::Notice => {
            let text = format!("{}\n{}", modal.title, modal.body);
            frame.render_widget(Paragraph::new(text).alignment(Alignment::Center), inner);
        }
        AttachModalKind::LineInput { textarea } => {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Length(1)])
                .split(inner);
            frame.render_widget(
                Paragraph::new(modal.body.as_str()).alignment(Alignment::Center),
                rows[0],
            );
            frame.render_widget(textarea.as_ref(), rows[1]);
        }
    }
}

fn modal_height(modal: &AttachModal) -> u16 {
    match &modal.kind {
        AttachModalKind::LineInput { .. } => 4,
        AttachModalKind::Confirm(_) | AttachModalKind::Notice => 5,
    }
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn textarea_input(key: KeyEvent) -> Input {
    let input_key = match key.code {
        KeyCode::Char(value) => Key::Char(value),
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::Tab => Key::Tab,
        _ => Key::Null,
    };
    Input {
        key: input_key,
        ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
        alt: key.modifiers.contains(KeyModifiers::ALT),
        shift: key.modifiers.contains(KeyModifiers::SHIFT),
    }
}

pub(crate) fn chain_attach_header_text(chain: &Chain) -> String {
    format!(
        "{}\npolicy branch={} apply={} strategy={} on-fail={}\nbase {}@{}  cwd {}",
        chain_attach_summary_line(chain),
        branch_policy_label(chain.branch_policy),
        apply_mode_label(chain.apply_mode),
        apply_strategy_label(chain_apply_strategy(chain)),
        on_fail_label(chain.on_fail),
        chain.base_branch,
        short_sha(&chain.base_sha),
        one_line(&chain.cwd.display().to_string(), 96)
    )
}

pub(crate) fn chain_timeline_lines(
    chain: &Chain,
    tui_state: &ChainAttachTuiState,
) -> Vec<Line<'static>> {
    chain
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let marker = if index == tui_state.selected_step {
                ">"
            } else {
                " "
            };
            let run = step
                .run_id
                .as_deref()
                .map(|run_id| format!(" run {}", run_prefix(run_id)))
                .unwrap_or_default();
            let mut spans = vec![
                Span::styled(marker.to_string(), Style::default().fg(Color::Cyan)),
                Span::raw(format!(
                    " {} step {:>2} {:<8} {}{}",
                    chain_step_dot(step.status),
                    step.index + 1,
                    chain_step_status_label(step.status),
                    one_line(&step.goal, 54),
                    run
                )),
            ];
            if let Some(reason) = step.fail_reason.as_deref() {
                spans.push(Span::styled(
                    format!("  {}", one_line(reason, 32)),
                    Style::default().fg(Color::Red),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

pub(crate) fn chain_activity_lines(
    events: &[ChainEvent],
    tui_state: &ChainAttachTuiState,
) -> Vec<Line<'static>> {
    let start = usize::from(tui_state.events_scroll).min(events.len());
    events
        .iter()
        .rev()
        .skip(start)
        .take(240)
        .map(|event| {
            let step = event
                .step_index
                .map(|index| format!(" step {}", index + 1))
                .unwrap_or_default();
            let detail = if event.detail.is_null() {
                String::new()
            } else {
                format!(" {}", one_line(&event.detail.to_string(), 120))
            };
            Line::from(format!(
                "{} {}{}{}",
                event.timestamp.format("%H:%M:%S"),
                chain_event_label(&event.event),
                step,
                detail
            ))
        })
        .collect()
}

pub(crate) fn chain_attach_footer_text(chain: &Chain) -> String {
    if chain.status == ChainStatus::Paused {
        let surface = chain_paused_attach_footer_surface(chain);
        let reason = chain.paused_reason.as_deref().unwrap_or("paused");
        let evidence = surface
            .explanation
            .evidence
            .iter()
            .map(|(key, value)| format!("{key} {value}"))
            .collect::<Vec<_>>()
            .join("; ");
        let other = surface
            .secondary_actions
            .iter()
            .map(|action| action.command.clone())
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "{} | why {reason} | evidence {evidence} | next {} | other {other} | q detach",
            surface.label(),
            surface.primary_action.command
        )
    } else {
        footer(&[
            ("[Enter]", "drill"),
            ("[r]", "redo"),
            ("[e]", "extend"),
            ("[p]", "pause"),
            ("[k]", "kill"),
            ("[Ctrl-D/q/Esc]", "detach"),
            ("j/k", "move"),
            ("PgUp/PgDn", "activity"),
            ("?", "help"),
        ])
    }
}

fn chain_paused_attach_footer_surface(chain: &Chain) -> VerdictSurface {
    let id = run_prefix(&chain.chain_id);
    let reason = chain.paused_reason.as_deref().unwrap_or("paused");
    VerdictSurface::must_new(
        VerdictKind::Paused,
        "chain",
        Some(&id),
        ExplanationPanel::new(
            "The chain is paused and no child run is advancing.",
            format!("{reason} is recorded as the pause reason."),
            [
                ("status", format!("{:?}", chain.status)),
                ("paused_reason", reason.to_string()),
            ],
        ),
        [("Recommended", format!("deadreckon chain resume {id}"))],
        [
            (
                "Inspect",
                format!("deadreckon chain show {id} --why-failed"),
            ),
            (
                "Preview",
                format!("deadreckon chain resume {id} --apply-mode preview"),
            ),
            ("Undo", format!("deadreckon chain undo {id}")),
        ],
    )
}

fn chain_event_label(event: &ChainEventKind) -> &'static str {
    match event {
        ChainEventKind::ChainCreated => "created",
        ChainEventKind::ChainStepStarted => "step started",
        ChainEventKind::ChainRunCompleted => "run completed",
        ChainEventKind::ChainApplyStarted => "apply started",
        ChainEventKind::ChainApplied => "applied",
        ChainEventKind::ChainApplyRefused => "apply refused",
        ChainEventKind::ChainStepFailed => "step failed",
        ChainEventKind::ChainPaused => "paused",
        ChainEventKind::ChainResumed => "resumed",
        ChainEventKind::ChainKilled => "killed",
        ChainEventKind::ChainCompleted => "completed",
        ChainEventKind::ChainUndoStarted => "undo started",
        ChainEventKind::ChainUndoneStep => "undone step",
        ChainEventKind::ChainHookInvoked => "hook",
        ChainEventKind::ChainStepExtended => "extended",
        ChainEventKind::ChainStepRedone => "redone",
    }
}
