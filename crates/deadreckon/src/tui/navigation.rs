use crossterm::event::{KeyCode, KeyEvent};

/// Shared navigation core for every attach surface (run, plan, campaign, chain).
///
/// The run panel's key semantics are the reference. Each surface implements the
/// movement hooks for its own content model (multi-panel scroll, list selection,
/// step graph, …) and supplies `mode_key` for its mode-specific keys. The common
/// navigation keys — arrows/`jk`, `Tab`/`BackTab`, `PgUp`/`PgDn`, `Home`/`End`,
/// `g`/`G` — are routed identically for everyone by [`dispatch_navigation`].
pub(crate) trait NavigableSurface {
    /// `Tab`: move focus to the next panel (no-op for single-panel surfaces).
    fn focus_next(&mut self) {}
    /// `BackTab`: move focus to the previous panel.
    fn focus_previous(&mut self) {}
    /// `Up`/`k` (delta -1) and `Down`/`j` (delta +1).
    fn scroll_lines(&mut self, delta: isize);
    /// `PgUp` (direction -1) and `PgDn` (direction +1).
    fn scroll_page(&mut self, direction: isize);
    /// `Home`/`g`.
    fn scroll_to_start(&mut self);
    /// `End`/`G`.
    fn scroll_to_end(&mut self);
    /// Handle a key the shared core does not own. Return `true` if consumed.
    fn mode_key(&mut self, _key: KeyEvent) -> bool {
        false
    }
}

/// What a key means for the `?` help overlay. One shared rule for every
/// attach surface: `?` opens, any key while open closes, everything else
/// flows through to the surface's normal handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HelpKeyAction {
    Open,
    Close,
    NotHandled,
}

pub(crate) fn handle_help_key(help_open: bool, key: KeyEvent) -> HelpKeyAction {
    if help_open {
        return HelpKeyAction::Close;
    }
    if key.code == KeyCode::Char('?') {
        return HelpKeyAction::Open;
    }
    HelpKeyAction::NotHandled
}

/// Route `key` through the shared navigation keys, falling back to the surface's
/// mode-specific hook. Returns `true` if the key was handled.
pub(crate) fn dispatch_navigation<S: NavigableSurface + ?Sized>(
    surface: &mut S,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Tab => surface.focus_next(),
        KeyCode::BackTab => surface.focus_previous(),
        KeyCode::Up | KeyCode::Char('k') => surface.scroll_lines(-1),
        KeyCode::Down | KeyCode::Char('j') => surface.scroll_lines(1),
        KeyCode::PageUp => surface.scroll_page(-1),
        KeyCode::PageDown => surface.scroll_page(1),
        KeyCode::Home | KeyCode::Char('g') => surface.scroll_to_start(),
        KeyCode::End | KeyCode::Char('G') => surface.scroll_to_end(),
        _ => return surface.mode_key(key),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[derive(Default)]
    struct Recorder {
        log: Vec<&'static str>,
        mode_keys: Vec<KeyCode>,
    }

    impl NavigableSurface for Recorder {
        fn focus_next(&mut self) {
            self.log.push("focus_next");
        }
        fn focus_previous(&mut self) {
            self.log.push("focus_previous");
        }
        fn scroll_lines(&mut self, delta: isize) {
            self.log
                .push(if delta < 0 { "line_up" } else { "line_down" });
        }
        fn scroll_page(&mut self, direction: isize) {
            self.log.push(if direction < 0 {
                "page_up"
            } else {
                "page_down"
            });
        }
        fn scroll_to_start(&mut self) {
            self.log.push("to_start");
        }
        fn scroll_to_end(&mut self) {
            self.log.push("to_end");
        }
        fn mode_key(&mut self, key: KeyEvent) -> bool {
            self.mode_keys.push(key.code);
            false
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    #[test]
    fn shared_dispatcher_routes_common_navigation_keys() {
        for (code, expected) in [
            (KeyCode::Tab, "focus_next"),
            (KeyCode::BackTab, "focus_previous"),
            (KeyCode::Up, "line_up"),
            (KeyCode::Char('k'), "line_up"),
            (KeyCode::Down, "line_down"),
            (KeyCode::Char('j'), "line_down"),
            (KeyCode::PageUp, "page_up"),
            (KeyCode::PageDown, "page_down"),
            (KeyCode::Home, "to_start"),
            (KeyCode::Char('g'), "to_start"),
            (KeyCode::End, "to_end"),
            (KeyCode::Char('G'), "to_end"),
        ] {
            let mut surface = Recorder::default();
            assert!(
                dispatch_navigation(&mut surface, key(code)),
                "{code:?} should be handled by the shared core"
            );
            assert_eq!(
                surface.log,
                vec![expected],
                "{code:?} routed to the wrong hook"
            );
        }

        // A non-navigation key falls through to mode_key and is reported unhandled.
        let mut surface = Recorder::default();
        assert!(!dispatch_navigation(&mut surface, key(KeyCode::Char('x'))));
        assert_eq!(surface.mode_keys, vec![KeyCode::Char('x')]);
        assert!(surface.log.is_empty());
    }

    #[test]
    fn question_mark_opens_help_and_any_key_closes_it() {
        assert_eq!(
            handle_help_key(false, key(KeyCode::Char('?'))),
            HelpKeyAction::Open
        );
        // SHIFT is how most keyboards type '?' — it must still open.
        assert_eq!(
            handle_help_key(
                false,
                KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT)
            ),
            HelpKeyAction::Open
        );
        for code in [
            KeyCode::Char('q'),
            KeyCode::Esc,
            KeyCode::Char('?'),
            KeyCode::Enter,
            KeyCode::Char('x'),
        ] {
            assert_eq!(
                handle_help_key(true, key(code)),
                HelpKeyAction::Close,
                "{code:?} must close the open overlay instead of acting"
            );
        }
        assert_eq!(
            handle_help_key(false, key(KeyCode::Char('q'))),
            HelpKeyAction::NotHandled
        );
    }
}
