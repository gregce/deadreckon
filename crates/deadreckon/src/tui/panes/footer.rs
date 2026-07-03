/// The single footer builder for every attach surface: a uniform
/// "<keys> <label>" affordance list joined by one separator. Each surface
/// supplies its own mode-specific items; the shape is identical everywhere.
pub(crate) use crate::tui::render::footer_for_state;

pub(crate) fn footer<S: AsRef<str>>(items: &[(S, S)]) -> String {
    items
        .iter()
        .map(|(keys, label)| {
            let (keys, label) = (keys.as_ref(), label.as_ref());
            if label.is_empty() {
                keys.to_string()
            } else {
                format!("{keys} {label}")
            }
        })
        .collect::<Vec<_>>()
        .join("  |  ")
}
