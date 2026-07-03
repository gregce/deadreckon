use crate::tui::panes::activity::attach_activity_lines_for_tui;
use crate::{
    AttachLive, AttachTuiState, SpendRecord, TraceRecord, acceptance_status_value, one_line,
    read_jsonl, run_prefix, run_spend_label,
};
use deadreckon_core::{PipelineState, RUN_EVENTS_JSONL, RunEvent};

pub(crate) fn run_detail_lines(state: &PipelineState, width: usize) -> Vec<String> {
    let spend = read_jsonl::<SpendRecord>(&state.run_root.join("spend.jsonl")).unwrap_or_default();
    let traces =
        read_jsonl::<TraceRecord>(&state.run_root.join("traces.jsonl")).unwrap_or_default();
    let events = read_jsonl::<RunEvent>(&state.run_root.join(RUN_EVENTS_JSONL)).unwrap_or_default();
    let mut lines = vec![
        format!(
            "run {}  status {}  turn {}",
            run_prefix(&state.run_id),
            state.status,
            state.turn
        ),
        one_line(&state.goal, width),
        format!(
            "spend {}  gate {}",
            run_spend_label(state, false),
            acceptance_status_value(state)
        ),
    ];
    lines.extend(
        attach_activity_lines_for_tui(
            state,
            &spend,
            &traces,
            &events,
            &AttachLive::default(),
            &AttachTuiState::default(),
        )
        .into_iter()
        .take(10)
        .map(|line| one_line(&line, width)),
    );
    lines
}
