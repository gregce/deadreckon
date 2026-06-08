mod attach_state;
pub(crate) mod navigation;
mod render;

pub(crate) use attach_state::{
    AttachActionNotice, AttachCampaignParent, AttachPanel, AttachParentPlan, AttachTuiState,
    attach_panel_counts, attach_panel_layout, toggle_attach_view,
};

#[cfg(test)]
pub(crate) use attach_state::{AttachPanelCounts, AttachPanelRows, max_panel_scroll};
pub(crate) use render::{
    ChainAttachTuiState, PLAN_NARRATIVE_AREA_HEIGHT, PlanAttachRenderState,
    RunNarrativeRenderInput, attach_activity_lines_for_tui, build_run_narrative_projection,
    chain_event_read_hint, ensure_run_narrative_projection, live_file_lines, plan_event_line,
    plan_event_summary, plan_final_gate_line, plan_provider_summary, plan_repair_label,
    plan_task_detail_lines, process_lines, provider_is_metered, render_attach,
    render_campaign_attach, render_chain_attach, render_plan_attach, run_narrative_projection,
    run_narrative_projection_signature,
};

#[cfg(test)]
pub(crate) use render::{
    CAMPAIGN_EMPTY_HINT, NARRATIVE_SPLIT_WIDTH, acceptance_activity_lines, attach_header_text,
    chain_activity_lines, chain_attach_footer_text, chain_attach_header_text, chain_timeline_lines,
    deadreckoning_status_text, footer, markdown_to_tui_lines, meter_color, panel_title,
    plan_attach_footer, plan_narrative_title, render_campaign_attach_text, scroll_indicator,
    selection_glyph, threshold_color,
};
