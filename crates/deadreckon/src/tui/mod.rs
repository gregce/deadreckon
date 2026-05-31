mod attach_state;

pub(crate) use attach_state::{
    AttachActionNotice, AttachPanel, AttachParentPlan, AttachTuiState, attach_panel_counts,
    attach_panel_layout, toggle_attach_view,
};

#[cfg(test)]
pub(crate) use attach_state::{AttachPanelCounts, AttachPanelRows, max_panel_scroll};
