mod attach_state;
mod render;

pub(crate) use attach_state::{
    AttachActionNotice, AttachPanel, AttachParentPlan, AttachTuiState, attach_panel_counts,
    attach_panel_layout, toggle_attach_view,
};

#[cfg(test)]
pub(crate) use attach_state::{AttachPanelCounts, AttachPanelRows, max_panel_scroll};
pub(crate) use render::{
    attach_activity_lines_for_tui, live_file_lines, markdown_to_tui_lines, narrative_list_item,
    panel_border_style, panel_title, process_lines, provider_is_metered, render_attach,
    visible_narrative_items,
};

#[cfg(test)]
pub(crate) use render::{
    acceptance_activity_lines, attach_header_text, deadreckoning_status_text, meter_color,
    threshold_color,
};
