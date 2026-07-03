#[cfg(test)]
pub(crate) use crate::tui::render::acceptance_activity_lines;
pub(crate) use crate::tui::render::{
    attach_activity_lines_for_tui, list_scroll_offset, live_file_lines, panel_border_style,
    panel_title, process_lines, render_live_files, render_processes, scroll_indicator,
    selection_glyph, visible_items,
};
