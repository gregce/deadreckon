use std::time::Duration;

use crate::commands::attach_runtime::AttachTickBudget;
use crate::commands::chain::{
    apply_mode_label, apply_strategy_label, branch_policy_label, chain_apply_strategy,
    chain_attach_summary_line, chain_step_dot, chain_step_status_label, on_fail_label, short_sha,
};
use crate::tui::effects::{
    EffectFrame, EffectFrameDecision, EffectRegistry, EffectTrigger, MotionPolicy, UiEffectEvent,
    registered_effect_triggers,
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
use serde_json::json;
use tui_textarea::{Input, Key, TextArea};

use crate::{CliError, one_line, run_prefix};

#[derive(Debug, Default)]
pub(crate) struct ChainAttachTuiState {
    pub(crate) selected_step: usize,
    pub(crate) events_scroll: u16,
    pub(crate) narrative_open: bool,
    pub(crate) narrative_scroll: usize,
    pub(crate) motion_policy: MotionPolicy,
    active_effect_frames: Vec<EffectFrame>,
    last_effect_status: Option<ChainStatus>,
    last_step_status_signature: Option<String>,
    pub(crate) discoverability_hint_visible: bool,
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

    fn confirm_command(invocation: CommandModeInvocation) -> Self {
        let target = invocation
            .target
            .as_deref()
            .map(|target| format!(" {target}"))
            .unwrap_or_default();
        Self {
            title: format!("{}{}?", invocation.verb.as_str(), target),
            body: "y confirm    n/Esc cancel".to_string(),
            kind: AttachModalKind::Confirm(ConfirmAction::Command(invocation)),
        }
    }

    fn line_input(
        title: impl Into<String>,
        body: impl Into<String>,
        placeholder: impl Into<String>,
        purpose: LineInputPurpose,
    ) -> Self {
        let mut textarea = TextArea::default();
        textarea.set_placeholder_text(placeholder);
        textarea.set_cursor_line_style(Style::default());
        textarea.set_cursor_style(Style::default().fg(Color::Cyan));
        Self {
            title: title.into(),
            body: body.into(),
            kind: AttachModalKind::LineInput {
                purpose,
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
                        ConfirmAction::Command(invocation) => ChainModalAction::CommandConfirmed {
                            verb: invocation.verb,
                            target: invocation.target.clone(),
                        },
                    })
                }
                KeyCode::Esc | KeyCode::Char('n') => ModalKeyOutcome::Close,
                _ => ModalKeyOutcome::Keep,
            },
            AttachModalKind::LineInput { purpose, textarea } => match key.code {
                KeyCode::Enter => {
                    let value = textarea.lines().first().cloned().unwrap_or_default();
                    match purpose {
                        LineInputPurpose::Extend => {
                            ModalKeyOutcome::Action(ChainModalAction::ExtendSubmitted(value))
                        }
                        LineInputPurpose::Command => ModalKeyOutcome::Command(value),
                    }
                }
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
    LineInput {
        purpose: LineInputPurpose,
        textarea: Box<TextArea<'static>>,
    },
    Notice,
}

#[derive(Debug, Clone, Copy)]
enum LineInputPurpose {
    Extend,
    Command,
}

#[derive(Debug, Clone)]
enum ConfirmAction {
    Kill,
    Command(CommandModeInvocation),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChainModalAction {
    None,
    KillConfirmed,
    ExtendSubmitted(String),
    CommandConfirmed {
        verb: CommandModeVerb,
        target: Option<String>,
    },
    QuitRequested,
}

enum ModalKeyOutcome {
    Keep,
    Close,
    Action(ChainModalAction),
    Command(String),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AttachCommandSpec {
    pub(crate) verb: &'static str,
    pub(crate) cli_command: Option<&'static str>,
    kind: CommandModeVerb,
    confirm: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandModeVerb {
    Attach,
    Kill,
    Motion,
    Quit,
    Reshape,
    Resume,
    Verdict,
    Why,
}

impl CommandModeVerb {
    fn as_str(self) -> &'static str {
        match self {
            Self::Attach => "attach",
            Self::Kill => "kill",
            Self::Motion => "motion",
            Self::Quit => "q",
            Self::Reshape => "reshape",
            Self::Resume => "resume",
            Self::Verdict => "verdict",
            Self::Why => "why",
        }
    }
}

#[derive(Debug, Clone)]
struct CommandModeInvocation {
    verb: CommandModeVerb,
    target: Option<String>,
}

#[derive(Debug)]
struct CommandModeParseError {
    nearest: &'static str,
}

pub(crate) fn attach_command_table() -> &'static [AttachCommandSpec] {
    const TABLE: &[AttachCommandSpec] = &[
        AttachCommandSpec {
            verb: "attach",
            cli_command: Some("deadreckon attach"),
            kind: CommandModeVerb::Attach,
            confirm: true,
        },
        AttachCommandSpec {
            verb: "kill",
            cli_command: Some("deadreckon chain kill"),
            kind: CommandModeVerb::Kill,
            confirm: true,
        },
        AttachCommandSpec {
            verb: "motion",
            cli_command: None,
            kind: CommandModeVerb::Motion,
            confirm: false,
        },
        AttachCommandSpec {
            verb: "q",
            cli_command: None,
            kind: CommandModeVerb::Quit,
            confirm: false,
        },
        AttachCommandSpec {
            verb: "reshape",
            cli_command: Some("deadreckon reshape"),
            kind: CommandModeVerb::Reshape,
            confirm: true,
        },
        AttachCommandSpec {
            verb: "resume",
            cli_command: Some("deadreckon chain resume"),
            kind: CommandModeVerb::Resume,
            confirm: true,
        },
        AttachCommandSpec {
            verb: "verdict",
            cli_command: Some("deadreckon verdict"),
            kind: CommandModeVerb::Verdict,
            confirm: true,
        },
        AttachCommandSpec {
            verb: "why",
            cli_command: Some("deadreckon show --why-failed"),
            kind: CommandModeVerb::Why,
            confirm: true,
        },
    ];
    TABLE
}

impl ChainAttachTuiState {
    pub(crate) fn new(narrative_open: bool, motion_policy: MotionPolicy) -> Self {
        Self {
            narrative_open,
            motion_policy,
            discoverability_hint_visible: true,
            ..Self::default()
        }
    }

    pub(crate) fn clamp(&mut self, chain: &Chain) {
        if chain.steps.is_empty() {
            self.selected_step = 0;
            self.events_scroll = 0;
            return;
        }
        self.selected_step = self.selected_step.min(chain.steps.len() - 1);
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent, chain: &Chain) {
        if key.code == KeyCode::Char('n') && key.modifiers.is_empty() {
            self.narrative_open = !self.narrative_open;
            self.narrative_scroll = 0;
            self.clamp(chain);
            return;
        }
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
            LineInputPurpose::Extend,
        ));
    }

    pub(crate) fn open_command_input(&mut self) {
        self.modal = Some(AttachModal::line_input(
            "command",
            "Enter run    Esc cancel",
            ":kill, :resume, :verdict, :why, :reshape, :attach, :motion, :q",
            LineInputPurpose::Command,
        ));
    }

    pub(crate) fn open_notice(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.modal = Some(AttachModal::notice(title, body));
    }

    pub(crate) fn dismiss_discoverability_hint(&mut self) {
        self.discoverability_hint_visible = false;
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
                ModalKeyOutcome::Command(input) => self.handle_command_submission(&input),
            };
        }
        if key.modifiers == KeyModifiers::NONE {
            match key.code {
                KeyCode::Char(':') => {
                    self.open_command_input();
                    return ChainModalAction::None;
                }
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

    fn handle_command_submission(&mut self, input: &str) -> ChainModalAction {
        match parse_command_mode(input) {
            Ok(invocation) => match invocation.verb {
                CommandModeVerb::Kill => {
                    self.open_kill_confirm();
                    ChainModalAction::None
                }
                CommandModeVerb::Motion => {
                    let Some(target) = invocation.target.as_deref() else {
                        self.open_notice(
                            "motion",
                            format!("motion {}", self.motion_policy.as_str()),
                        );
                        return ChainModalAction::None;
                    };
                    match MotionPolicy::parse(target) {
                        Some(policy) => {
                            self.motion_policy = policy;
                            self.open_notice(
                                "motion",
                                format!("motion {} for this attach session", policy.as_str()),
                            );
                        }
                        None => {
                            self.open_notice("motion", "try: :motion full|reduced|off");
                        }
                    }
                    ChainModalAction::None
                }
                CommandModeVerb::Quit => ChainModalAction::QuitRequested,
                _ => {
                    let Some(spec) = attach_command_table()
                        .iter()
                        .find(|spec| spec.kind == invocation.verb)
                    else {
                        self.open_notice("unknown command", "try: :kill");
                        return ChainModalAction::None;
                    };
                    debug_assert!(spec.cli_command.is_some() || spec.kind == CommandModeVerb::Quit);
                    if spec.confirm {
                        self.modal = Some(AttachModal::confirm_command(invocation));
                        ChainModalAction::None
                    } else {
                        ChainModalAction::CommandConfirmed {
                            verb: invocation.verb,
                            target: invocation.target,
                        }
                    }
                }
            },
            Err(err) => {
                self.open_notice("unknown command", format!("try: :{}", err.nearest));
                ChainModalAction::None
            }
        }
    }

    pub(crate) fn scroll(&mut self, delta: isize, chain: &Chain) {
        if chain.steps.is_empty() {
            return;
        }
        let next = (self.selected_step as isize + delta)
            .clamp(0, chain.steps.len().saturating_sub(1) as isize);
        self.selected_step = next as usize;
    }

    pub(crate) fn effect_registry(&self) -> EffectRegistry {
        EffectRegistry::new(self.motion_policy)
    }

    pub(crate) fn refresh_effects_for_chain(&mut self, chain: &Chain, input_pending: bool) {
        debug_assert_eq!(registered_effect_triggers().len(), 3);
        debug_assert!(
            self.effect_registry()
                .frames_for_event(UiEffectEvent::Unregistered("chain-refresh"))
                .is_empty()
        );

        let mut next_frames = Vec::new();
        let step_signature = chain_step_status_signature(chain);
        if self
            .last_step_status_signature
            .as_deref()
            .is_some_and(|previous| previous != step_signature)
        {
            next_frames
                .extend(self.frames_for_trigger(EffectTrigger::NodeStateChange, input_pending));
        }
        self.last_step_status_signature = Some(step_signature);

        if self.last_effect_status.is_some_and(|previous| {
            previous != chain.status && chain_status_is_verdict(chain.status)
        }) {
            next_frames
                .extend(self.frames_for_trigger(EffectTrigger::VerdictCompletion, input_pending));
        }
        self.last_effect_status = Some(chain.status);
        self.active_effect_frames = next_frames;
    }

    pub(crate) fn clear_active_effect_frames(&mut self) {
        self.active_effect_frames.clear();
    }

    fn frames_for_trigger(&self, trigger: EffectTrigger, input_pending: bool) -> Vec<EffectFrame> {
        let registry = self.effect_registry();
        let event = UiEffectEvent::Registered(trigger);
        if registry.next_frame_decision(event, input_pending)
            == EffectFrameDecision::PreemptedForInput
        {
            Vec::new()
        } else {
            registry.frames_for_event(event)
        }
    }
}

fn chain_step_status_signature(chain: &Chain) -> String {
    chain
        .steps
        .iter()
        .map(|step| format!("{}:{:?};", step.index, step.status))
        .collect::<String>()
}

fn chain_status_is_verdict(status: ChainStatus) -> bool {
    matches!(
        status,
        ChainStatus::Completed | ChainStatus::Failed | ChainStatus::Killed | ChainStatus::Undone
    )
}

fn parse_command_mode(
    input: &str,
) -> std::result::Result<CommandModeInvocation, CommandModeParseError> {
    let normalized = input.trim().trim_start_matches(':').trim();
    let Some(raw_verb) = normalized.split_whitespace().next() else {
        return Err(CommandModeParseError { nearest: "kill" });
    };
    let spec = command_spec_for(raw_verb).ok_or_else(|| CommandModeParseError {
        nearest: nearest_command(raw_verb),
    })?;
    let target = normalized[raw_verb.len()..].trim();
    let target = if target.is_empty() {
        None
    } else {
        Some(target)
    };
    Ok(CommandModeInvocation {
        verb: spec.kind,
        target: target.map(ToString::to_string),
    })
}

fn command_spec_for(raw_verb: &str) -> Option<&'static AttachCommandSpec> {
    if let Some(exact) = attach_command_table()
        .iter()
        .find(|spec| spec.verb == raw_verb)
    {
        return Some(exact);
    }
    let mut matches = attach_command_table()
        .iter()
        .filter(|spec| spec.verb.starts_with(raw_verb));
    let first = matches.next()?;
    if matches.next().is_none() {
        Some(first)
    } else {
        None
    }
}

fn nearest_command(raw_verb: &str) -> &'static str {
    attach_command_table()
        .iter()
        .min_by_key(|spec| edit_distance(raw_verb, spec.verb))
        .map(|spec| spec.verb)
        .unwrap_or("kill")
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right_chars = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right_chars.len()).collect::<Vec<_>>();
    for (left_index, left_char) in left.chars().enumerate() {
        let mut current = vec![left_index + 1];
        for (right_index, right_char) in right_chars.iter().enumerate() {
            let insert = current[right_index] + 1;
            let delete = previous[right_index + 1] + 1;
            let replace = previous[right_index] + usize::from(left_char != *right_char);
            current.push(insert.min(delete).min(replace));
        }
        previous = current;
    }
    previous[right_chars.len()]
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
        if self.state.narrative_open {
            self.state.narrative_scroll = if delta < 0 {
                self.state
                    .narrative_scroll
                    .saturating_sub(delta.unsigned_abs())
            } else {
                self.state.narrative_scroll.saturating_add(delta as usize)
            };
        } else {
            self.state.scroll(delta, self.chain);
        }
    }

    fn scroll_page(&mut self, direction: isize) {
        if self.state.narrative_open {
            self.state.narrative_scroll = if direction < 0 {
                self.state.narrative_scroll.saturating_sub(8)
            } else {
                self.state.narrative_scroll.saturating_add(8)
            };
        } else if direction < 0 {
            self.state.events_scroll = self.state.events_scroll.saturating_sub(8)
        } else {
            self.state.events_scroll = self.state.events_scroll.saturating_add(8)
        };
    }

    fn scroll_to_start(&mut self) {
        if self.state.narrative_open {
            self.state.narrative_scroll = 0;
        } else {
            self.state.selected_step = 0;
            self.state.events_scroll = 0;
        }
    }

    fn scroll_to_end(&mut self) {
        if self.state.narrative_open {
            self.state.narrative_scroll = usize::MAX / 2;
        } else {
            self.state.selected_step = self.chain.steps.len().saturating_sub(1);
        }
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
            "voyage{}",
            scroll_indicator(steps_offset, steps_rows, steps_total)
        ))),
        body[0],
    );
    if tui_state.narrative_open {
        let lines = chain_narrative_lines(chain, events);
        let rows = body[1].height.saturating_sub(2) as usize;
        let scroll = tui_state
            .narrative_scroll
            .min(lines.len().saturating_sub(rows.max(1)));
        let visible = lines
            .into_iter()
            .skip(scroll)
            .take(rows.max(1))
            .map(ListItem::new)
            .collect::<Vec<_>>();
        frame.render_widget(
            List::new(visible).block(Block::default().borders(Borders::ALL).title(format!(
                "chain narrative{}",
                scroll_indicator(scroll, rows, chain_narrative_lines(chain, events).len())
            ))),
            body[1],
        );
    } else {
        let event_lines = chain_activity_lines(events, tui_state)
            .into_iter()
            .map(ListItem::new)
            .collect::<Vec<_>>();
        let activity_title = chain_activity_title(tui_state);
        frame.render_widget(
            List::new(event_lines)
                .block(Block::default().borders(Borders::ALL).title(activity_title)),
            body[1],
        );
    }
    let spine = spine_for_chain_with_events(chain, events, Utc::now());
    render_spine_band(frame, rows[2], &spine);
    frame.render_widget(
        Paragraph::new(chain_attach_footer_text_for_state(chain, tui_state)),
        rows[3],
    );
    render_active_effects(frame, rows[0], body[0], body[1], tui_state);
    if let Some(modal) = &tui_state.modal {
        render_chain_modal(frame, modal);
    }
}

fn render_active_effects(
    frame: &mut ratatui::Frame<'_>,
    gate_area: Rect,
    node_area: Rect,
    verdict_area: Rect,
    tui_state: &ChainAttachTuiState,
) {
    for effect_frame in &tui_state.active_effect_frames {
        let area = match effect_frame.trigger {
            EffectTrigger::GatePass => gate_area,
            EffectTrigger::VerdictCompletion => verdict_area,
            EffectTrigger::NodeStateChange => node_area,
        };
        let _effect = effect_frame.tachyon_effect();
        let color = match effect_frame.trigger {
            EffectTrigger::GatePass => Color::Cyan,
            EffectTrigger::VerdictCompletion => Color::Yellow,
            EffectTrigger::NodeStateChange => Color::Magenta,
        };
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(color)),
            area,
        );
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
        AttachModalKind::LineInput { textarea, .. } => {
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

pub(crate) fn chain_narrative_text_lines(chain: &Chain, events: &[ChainEvent]) -> Vec<String> {
    let mut lines = vec![
        "chain narrative".to_string(),
        format!("root goal {}", chain.root_goal),
        format!("summary {}", chain_attach_summary_line(chain)),
    ];
    if let Some(step) = chain.steps.first() {
        lines.push(format!(
            "current step {} {} {}",
            step.index + 1,
            chain_step_status_label(step.status),
            one_line(&step.goal, 96)
        ));
    }
    lines.push(String::new());
    lines.push("steps".to_string());
    for step in &chain.steps {
        let run = step
            .run_id
            .as_deref()
            .map(|run_id| format!(" run {}", run_prefix(run_id)))
            .unwrap_or_default();
        let reason = step
            .fail_reason
            .as_deref()
            .map(|reason| format!(" reason {}", one_line(reason, 72)))
            .unwrap_or_default();
        lines.push(format!(
            "step {} {} {}{}{}",
            step.index + 1,
            chain_step_status_label(step.status),
            one_line(&step.goal, 96),
            run,
            reason
        ));
    }
    lines.push(String::new());
    lines.push("recent activity".to_string());
    if events.is_empty() {
        lines.push("no chain events recorded yet".to_string());
    } else {
        lines.extend(
            chain_activity_lines(events, &ChainAttachTuiState::default())
                .into_iter()
                .take(8)
                .map(|line| {
                    line.spans
                        .iter()
                        .map(|span| span.content.as_ref())
                        .collect::<Vec<_>>()
                        .join("")
                }),
        );
    }
    lines
}

pub(crate) fn chain_narrative_lines(chain: &Chain, events: &[ChainEvent]) -> Vec<Line<'static>> {
    chain_narrative_text_lines(chain, events)
        .into_iter()
        .map(Line::from)
        .collect()
}

pub(crate) fn chain_narrative_plain_text(chain: &Chain, events: &[ChainEvent]) -> String {
    let mut output = chain_narrative_text_lines(chain, events).join("\n");
    output.push('\n');
    output
}

pub(crate) fn chain_narrative_json_text(
    chain: &Chain,
    events: &[ChainEvent],
) -> crate::Result<String> {
    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&json!({
            "status": "supported",
            "kind": "chain",
            "id": chain.chain_id,
            "lines": chain_narrative_text_lines(chain, events),
            "snapshot": {
                "chain_id": chain.chain_id,
                "root_goal": chain.root_goal,
                "status": crate::commands::chain::chain_status_label(chain),
                "steps": chain.steps.iter().map(|step| {
                    json!({
                        "index": step.index + 1,
                        "goal": step.goal,
                        "status": chain_step_status_label(step.status),
                        "run_id": step.run_id,
                    })
                }).collect::<Vec<_>>()
            }
        }))?
    ))
}

#[cfg(test)]
pub(crate) fn chain_attach_footer_text(chain: &Chain) -> String {
    chain_attach_footer_text_for_state(chain, &ChainAttachTuiState::default())
}

fn chain_attach_footer_text_for_state(chain: &Chain, tui_state: &ChainAttachTuiState) -> String {
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
    } else if tui_state.narrative_open {
        let mut items = Vec::new();
        items.push(("[n]", "Activity"));
        if tui_state.discoverability_hint_visible {
            items.push(("Tab panes · w why · : commands", ""));
        }
        items.extend([
            ("[Enter]", "drill"),
            ("[Ctrl-D/q/Esc]", "detach"),
            ("j/k PgUp/PgDn", "scroll"),
        ]);
        items.push(("?", "help"));
        footer(&items)
    } else {
        let mut items = Vec::new();
        items.extend([("[n]", "Narrative"), (":", "commands")]);
        if tui_state.discoverability_hint_visible {
            items.push(("Tab panes · w why · : commands", ""));
        }
        items.extend([
            ("[Enter]", "drill"),
            ("[r]", "redo"),
            ("[e]", "extend"),
            ("[p]", "pause"),
            ("[k]", "kill"),
            ("[Ctrl-D/q/Esc]", "detach"),
            ("j/k", "move"),
            ("PgUp/PgDn", "activity"),
        ]);
        items.push(("?", "help"));
        footer(&items)
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
        ChainEventKind::LegacyExecutionSelected => "legacy execution",
    }
}
