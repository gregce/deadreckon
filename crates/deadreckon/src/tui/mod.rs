mod attach_state;
mod render;

pub(crate) use attach_state::{
    AttachActionNotice, AttachPanel, AttachParentPlan, AttachTuiState, attach_panel_counts,
    attach_panel_layout, toggle_attach_view,
};

#[cfg(test)]
pub(crate) use attach_state::{AttachPanelCounts, AttachPanelRows, max_panel_scroll};
pub(crate) use render::{
    ChainAttachTuiState, PlanAttachRenderState, RunNarrativeRenderInput,
    attach_activity_lines_for_tui, build_run_narrative_projection, chain_event_read_hint,
    ensure_run_narrative_projection, live_file_lines, plan_event_line, plan_event_summary,
    plan_final_gate_line, plan_provider_summary, plan_repair_label, plan_task_detail_lines,
    process_lines, provider_is_metered, render_attach, render_chain_attach, render_plan_attach,
    run_narrative_projection, run_narrative_projection_signature,
};

#[cfg(test)]
pub(crate) use render::{
    acceptance_activity_lines, attach_header_text, chain_activity_lines, chain_attach_footer_text,
    chain_attach_header_text, chain_timeline_lines, deadreckoning_status_text,
    markdown_to_tui_lines, meter_color, plan_attach_footer, threshold_color,
};
