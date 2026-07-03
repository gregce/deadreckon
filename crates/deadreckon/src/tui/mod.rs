mod attach_state;
pub(crate) mod navigation;
pub(crate) mod panes;
mod render;
pub(crate) mod spine;
pub(crate) mod surfaces;
pub(crate) mod tree;
pub(crate) mod why;

pub(crate) use attach_state::{
    AttachActionNotice, AttachCampaignParent, AttachPanel, AttachParentPlan, AttachTuiState,
    attach_panel_counts, attach_panel_layout, toggle_attach_view,
};

#[cfg(test)]
pub(crate) use attach_state::{AttachPanelCounts, AttachPanelRows, max_panel_scroll};
pub(crate) use panes::activity::{attach_activity_lines_for_tui, live_file_lines, process_lines};
pub(crate) use panes::header::provider_is_metered;
pub(crate) use panes::narrative::{
    PLAN_NARRATIVE_AREA_HEIGHT, RunNarrativeRenderInput, build_run_narrative_projection,
    ensure_run_narrative_projection, run_narrative_projection, run_narrative_projection_signature,
};
pub(crate) use render::{AttachHelpMode, render_help_overlay};
pub(crate) use surfaces::campaign::render_campaign_attach;
pub(crate) use surfaces::chain::{
    ChainAttachTuiState, ChainModalAction, CommandModeVerb, chain_event_read_hint,
    render_chain_attach,
};
pub(crate) use surfaces::plan::{
    PlanAttachRenderState, plan_event_line, plan_event_summary, plan_final_gate_line,
    plan_provider_summary, plan_repair_label, plan_task_detail_lines, render_plan_attach,
};
pub(crate) use surfaces::run::render_attach;
pub(crate) use why::{why_for_run, why_plain_lines};

#[cfg(test)]
pub(crate) use why::render_why_panel;

#[cfg(test)]
pub(crate) use render::help_overlay_lines;

#[cfg(test)]
pub(crate) use panes::activity::{
    acceptance_activity_lines, panel_title, scroll_indicator, selection_glyph,
};
#[cfg(test)]
pub(crate) use panes::docs::markdown_to_tui_lines;
#[cfg(test)]
pub(crate) use panes::footer::footer;
#[cfg(test)]
pub(crate) use panes::header::{
    attach_header_text, deadreckoning_status_text, meter_color, threshold_color,
};
#[cfg(test)]
pub(crate) use panes::narrative::NARRATIVE_SPLIT_WIDTH;
#[cfg(test)]
pub(crate) use surfaces::campaign::{CAMPAIGN_EMPTY_HINT, render_campaign_attach_text};
#[cfg(test)]
pub(crate) use surfaces::chain::{
    attach_command_table, chain_activity_lines, chain_attach_footer_text, chain_attach_header_text,
    chain_timeline_lines,
};
#[cfg(test)]
pub(crate) use surfaces::plan::{plan_attach_footer, plan_narrative_title};
