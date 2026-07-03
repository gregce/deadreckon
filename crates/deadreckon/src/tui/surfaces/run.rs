use crate::tui::attach_state::{AttachPanel, AttachTuiState, attach_panel_layout};
use crate::tui::panes::activity::{
    attach_activity_lines_for_tui, panel_border_style, panel_title, render_live_files,
    render_processes, visible_items,
};
use crate::tui::panes::docs::render_run_docs;
use crate::tui::panes::footer::footer_for_state;
use crate::tui::panes::header::{
    attach_header_text_for_state, deadreckoning_status_line, provider_is_metered,
    render_acceptance, render_context, render_spend,
};
use crate::tui::panes::narrative::{RunNarrativeRenderInput, render_run_narrative};
use crate::tui::render::turn_timer;
use crate::tui::spine::{render_spine_band, spine_for_run_with_events};
use crate::{AttachLive, RunEvent, SpendRecord, TraceRecord};
use chrono::Utc;
use deadreckon_core::RunStatus;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::widgets::{Block, Borders, List, Paragraph};

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
    let spine = spine_for_run_with_events(state, events, Utc::now());
    render_spine_band(frame, layout.spine, &spine);
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
            ratatui::text::Line::from(footer_for_state(state, tui_state)),
        ]),
        layout.footer,
    );
}
