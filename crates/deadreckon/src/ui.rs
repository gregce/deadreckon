use std::io::{self, IsTerminal, Write as _};
use std::iter::Peekable;
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::style::Color;

static PLAIN_OUTPUT: AtomicBool = AtomicBool::new(false);
#[allow(dead_code)]
const ANSI_ESC: char = '\u{1b}';
const ANSI_RESET: &str = "\x1b[0m";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stream {
    Stdout,
    Stderr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    #[allow(dead_code)]
    Plain,
    Heading,
    Muted,
    Id,
    Command,
    Ok,
    Warn,
    Paused,
    Note,
    Negative,
    Prompt,
    Hint,
}

impl Tone {
    /// The single tone->ANSI SGR-parameter table for line output. `None` means
    /// the tone carries no color (plain text).
    pub(crate) const fn to_ansi(self) -> Option<&'static str> {
        match self {
            Tone::Plain => None,
            Tone::Heading => Some("1;36"),
            Tone::Muted => Some("2"),
            Tone::Id => Some("1;35"),
            Tone::Command => Some("1;34"),
            Tone::Ok => Some("1;32"),
            Tone::Warn => Some("1;33"),
            Tone::Paused => Some("1;33"),
            Tone::Note => Some("2"),
            Tone::Negative => Some("1;31"),
            Tone::Prompt => Some("1;36"),
            Tone::Hint => Some("1;34"),
        }
    }

    /// The single tone->ratatui::Color table for the TUI, derived from the same
    /// Tone enum so a status renders the same color on a line and in a frame.
    pub(crate) const fn to_tui_color(self) -> Color {
        match self {
            Tone::Plain => Color::Reset,
            Tone::Heading | Tone::Prompt => Color::Cyan,
            Tone::Muted | Tone::Note => Color::DarkGray,
            Tone::Id => Color::Magenta,
            Tone::Command | Tone::Hint => Color::Blue,
            Tone::Ok => Color::Green,
            Tone::Warn | Tone::Paused => Color::Yellow,
            Tone::Negative => Color::Red,
        }
    }
}

/// The closed set of lifecycle status classes. `status_tone` resolves a free-form
/// status string into exactly one of these — including an explicit `Unknown` —
/// so an unrecognized status renders in a visible default tone rather than being
/// silently folded into a dim catch-all, and so adding a class is a compile error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Ok,
    Running,
    Paused,
    Warn,
    Negative,
    Note,
    Unknown,
}

impl Status {
    pub(crate) fn classify(status: &str) -> Status {
        let status = status.trim().to_ascii_lowercase();
        if status.contains("failed")
            || status.contains("killed")
            || status.contains("error")
            || status.contains("missing")
            || status.contains("refused")
        {
            return Status::Negative;
        }
        if status.contains("paused") {
            return Status::Paused;
        }
        if status.contains("warning") || status.contains("warn") {
            return Status::Warn;
        }
        match status.as_str() {
            "ok" | "ready" | "set" | "wrote" | "updated" | "installed" | "completed" | "passed"
            | "polished" | "applied" | "cleaned" | "exported" => Status::Ok,
            "running" => Status::Running,
            "pending" | "planned" | "skipped" | "undone" | "recorded" | "note" => Status::Note,
            _ => Status::Unknown,
        }
    }

    pub(crate) const fn tone(self) -> Tone {
        match self {
            Status::Ok => Tone::Ok,
            Status::Running => Tone::Heading,
            Status::Paused => Tone::Paused,
            Status::Warn => Tone::Warn,
            Status::Negative => Tone::Negative,
            Status::Note => Tone::Note,
            // Unknown stays a visible default rather than a dim that hides it.
            Status::Unknown => Tone::Plain,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TuiPalette {
    pub(crate) border_focused: Color,
    pub(crate) border_idle: Color,
    pub(crate) status_running: Color,
    pub(crate) status_completed: Color,
    pub(crate) status_failed: Color,
    pub(crate) acceptance_default: Color,
    pub(crate) acceptance_configured: Color,
    pub(crate) acceptance_running: Color,
    pub(crate) acceptance_passed: Color,
    pub(crate) acceptance_failed: Color,
    pub(crate) spend_low: Color,
    pub(crate) spend_mid: Color,
    pub(crate) spend_high: Color,
    pub(crate) spend_pause_cap: Color,
}

pub(crate) const TUI_PALETTE: TuiPalette = TuiPalette {
    border_focused: Color::Cyan,
    border_idle: Color::Reset,
    // Derived from the shared Tone so line and TUI status colors cannot drift.
    status_running: Status::Running.tone().to_tui_color(),
    status_completed: Status::Ok.tone().to_tui_color(),
    status_failed: Status::Negative.tone().to_tui_color(),
    acceptance_default: Color::DarkGray,
    acceptance_configured: Color::Yellow,
    acceptance_running: Color::Cyan,
    acceptance_passed: Color::Green,
    acceptance_failed: Color::Red,
    spend_low: Color::Green,
    spend_mid: Color::Yellow,
    spend_high: Color::Red,
    spend_pause_cap: Color::Magenta,
};

pub(crate) fn set_plain_output(plain: bool) {
    PLAIN_OUTPUT.store(plain, Ordering::Relaxed);
}

pub(crate) fn enabled(stream: Stream) -> bool {
    if PLAIN_OUTPUT.load(Ordering::Relaxed) {
        return false;
    }
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if std::env::var("TERM").is_ok_and(|term| term == "dumb") {
        return false;
    }
    stream_is_terminal(stream)
}

pub(crate) fn render(stream: Stream, tone: Tone, text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    let Some(code) = tone.to_ansi() else {
        return text.to_string();
    };
    if enabled(stream) {
        ansi_wrap(code, text)
    } else {
        text.to_string()
    }
}

pub(crate) fn ansi_wrap(code: &str, text: &str) -> String {
    format!("\x1b[{code}m{text}{ANSI_RESET}")
}

#[allow(dead_code)]
pub(crate) fn ansi_reset() -> &'static str {
    ANSI_RESET
}

#[allow(dead_code)]
pub(crate) fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if read_ansi_sequence(ch, &mut chars).is_some() {
            continue;
        }
        out.push(ch);
    }
    out
}

#[allow(dead_code)]
pub(crate) fn read_ansi_sequence<I>(first: char, chars: &mut Peekable<I>) -> Option<(String, bool)>
where
    I: Iterator<Item = char>,
{
    if first != ANSI_ESC || chars.peek() != Some(&'[') {
        return None;
    }
    let mut sequence = String::from(first);
    if let Some(bracket) = chars.next() {
        sequence.push(bracket);
    }
    for code in chars.by_ref() {
        sequence.push(code);
        if code.is_ascii_alphabetic() {
            let active = sequence != ANSI_RESET;
            return Some((sequence, active));
        }
    }
    Some((sequence, false))
}

/// Display width of `text` in terminal columns: ANSI escape sequences are
/// stripped first, then each glyph is measured by its Unicode display width so
/// wide (CJK / full-width) glyphs count as two columns and zero-width / combining
/// marks count as zero. This is the single width function behind every pad,
/// truncate, and column-alignment site.
#[allow(dead_code)] // used by ui_card today; kv_block/columns/prompt sites adopt it in later phases.
pub(crate) fn display_width(text: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    UnicodeWidthStr::width(strip_ansi(text).as_str())
}

/// Right-pad `text` with spaces to `width` DISPLAY columns (ANSI-aware). Use this
/// for column alignment instead of `{:<N}`: a `{:<N}` format-pad counts the ANSI
/// escape bytes of a styled cell, so a colored column ends up short and misaligns
/// against its plain header.
#[allow(dead_code)] // call sites adopt it across P3/P5/P6.
pub(crate) fn pad_visible(text: &str, width: usize) -> String {
    let used = display_width(text);
    if used >= width {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(width - used))
    }
}

/// Word-wrap `text` to `width` display columns, breaking words longer than the
/// width. This is the single wrap engine behind the kv block, run list, and
/// campaign facts. Wrap RAW (unstyled) text — it is ANSI-naive.
#[allow(dead_code)] // call sites adopt it across P10.
pub(crate) fn wrap_words(text: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if display_width(word) > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            // Break a too-long word into width-sized chunks.
            let mut chunk = String::new();
            let mut chunk_width = 0usize;
            for ch in word.chars() {
                let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
                if chunk_width + ch_width > width {
                    lines.push(std::mem::take(&mut chunk));
                    chunk_width = 0;
                }
                chunk.push(ch);
                chunk_width += ch_width;
            }
            current = chunk;
            continue;
        }
        let next = if current.is_empty() {
            display_width(word)
        } else {
            display_width(&current) + 1 + display_width(word)
        };
        if next <= width {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

pub(crate) fn status_tone(status: impl AsRef<str>) -> Tone {
    Status::classify(status.as_ref()).tone()
}

pub(crate) fn render_status(stream: Stream, status: impl AsRef<str>) -> String {
    let status = status.as_ref();
    render(stream, status_tone(status), status)
}

pub(crate) fn ui_heading(text: impl AsRef<str>) -> String {
    render(Stream::Stdout, Tone::Heading, text)
}

pub(crate) fn ui_muted(text: impl AsRef<str>) -> String {
    render(Stream::Stdout, Tone::Muted, text)
}

pub(crate) fn ui_id(text: impl AsRef<str>) -> String {
    render(Stream::Stdout, Tone::Id, text)
}

pub(crate) fn ui_command(text: impl AsRef<str>) -> String {
    render(Stream::Stdout, Tone::Command, text)
}

pub(crate) fn ui_ok(text: impl AsRef<str>) -> String {
    render(Stream::Stdout, Tone::Ok, text)
}

pub(crate) fn ui_warn(text: impl AsRef<str>) -> String {
    render(Stream::Stdout, Tone::Warn, text)
}

pub(crate) fn ui_status(text: impl AsRef<str>) -> String {
    render_status(Stream::Stdout, text)
}

pub(crate) fn ui_error(text: impl AsRef<str>) -> String {
    render(Stream::Stderr, Tone::Negative, text)
}

pub(crate) fn writeln(stream: Stream, tone: Tone, text: impl AsRef<str>) -> io::Result<()> {
    let rendered = render(stream, tone, text);
    match stream {
        Stream::Stdout => {
            println!("{rendered}");
            io::stdout().flush()
        }
        Stream::Stderr => {
            eprintln!("{rendered}");
            io::stderr().flush()
        }
    }
}

pub(crate) fn hint(stream: Stream, text: impl AsRef<str>) -> io::Result<()> {
    writeln(
        stream,
        Tone::Hint,
        format!("  hint: {}", text.as_ref().trim()),
    )
}

pub(crate) fn clear_current_line(stream: Stream) -> io::Result<()> {
    if !stream_is_terminal(stream) {
        return Ok(());
    }
    match stream {
        Stream::Stdout => {
            print!("\r\x1b[K");
            io::stdout().flush()
        }
        Stream::Stderr => {
            eprint!("\r\x1b[K");
            io::stderr().flush()
        }
    }
}

pub(crate) fn replace_current_line(stream: Stream, text: impl AsRef<str>) -> io::Result<()> {
    if !stream_is_terminal(stream) {
        return Ok(());
    }
    let text = replacement_line_text(text.as_ref(), current_terminal_width());
    match stream {
        Stream::Stdout => {
            print!("\r{text}\x1b[K");
            io::stdout().flush()
        }
        Stream::Stderr => {
            eprint!("\r{text}\x1b[K");
            io::stderr().flush()
        }
    }
}

fn stream_is_terminal(stream: Stream) -> bool {
    match stream {
        Stream::Stdout => io::stdout().is_terminal(),
        Stream::Stderr => io::stderr().is_terminal(),
    }
}

fn current_terminal_width() -> usize {
    crossterm::terminal::size()
        .ok()
        .map(|(columns, _)| usize::from(columns))
        .or_else(|| {
            std::env::var("COLUMNS")
                .ok()
                .and_then(|columns| columns.parse::<usize>().ok())
        })
        .filter(|columns| *columns > 0)
        .unwrap_or(120)
}

fn replacement_line_text(text: &str, terminal_width: usize) -> String {
    let visible_limit = terminal_width.saturating_sub(1);
    truncate_visible_single_line(text, visible_limit)
}

fn truncate_visible_single_line(text: &str, max_visible: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    if max_visible == 0 {
        return String::new();
    }
    let mut out = String::with_capacity(text.len().min(max_visible));
    let mut chars = text.chars().peekable();
    let mut visible = 0usize;
    let mut style_active = false;
    while let Some(ch) = chars.next() {
        if let Some((sequence, active)) = read_ansi_sequence(ch, &mut chars) {
            style_active = active;
            out.push_str(&sequence);
            continue;
        }
        let glyph = match ch {
            '\n' | '\r' => ' ',
            _ => ch,
        };
        let glyph_width = UnicodeWidthChar::width(glyph).unwrap_or(0);
        if visible + glyph_width > max_visible {
            break;
        }
        out.push(glyph);
        visible += glyph_width;
    }
    if style_active {
        out.push_str(ANSI_RESET);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        Status, TUI_PALETTE, Tone, display_width, pad_visible, replacement_line_text, status_tone,
        strip_ansi, truncate_visible_single_line,
    };

    #[test]
    fn colored_column_aligns_same_with_color_on_and_off() {
        let plain = pad_visible("abc", 10);
        let colored = pad_visible("\x1b[1;35mabc\x1b[0m", 10);
        assert_eq!(display_width(&plain), 10);
        assert_eq!(display_width(&colored), 10);
        // Same trailing padding (7 = 10 - 3) regardless of styling.
        assert!(plain.ends_with("       "), "{plain:?}");
        assert!(colored.ends_with("       "), "{colored:?}");
    }

    #[test]
    fn one_wrap_engine_used_for_kv_list_campaign() {
        use super::wrap_words;
        assert_eq!(
            wrap_words("alpha beta gamma", 11),
            vec!["alpha beta", "gamma"]
        );
        assert_eq!(
            wrap_words("supercalifragilistic", 8),
            vec!["supercal", "ifragili", "stic"]
        );
        // Wide (CJK) glyphs count as two columns when wrapping.
        assert_eq!(wrap_words("中文 ok", 4), vec!["中文", "ok"]);
        assert_eq!(wrap_words("", 10), vec![String::new()]);
    }

    #[test]
    fn format_pad_over_ansi_is_rejected() {
        // The bug class: `{:<N}` counts ANSI escape bytes, so a styled cell and a
        // plain cell with identical visible text get different widths and the
        // column misaligns. pad_visible measures display columns instead.
        let plain = "abc";
        let styled = "\x1b[1;35mabc\x1b[0m";
        assert_ne!(
            display_width(&format!("{plain:<10}")),
            display_width(&format!("{styled:<10}")),
            "naive format-pad misaligns colored vs plain"
        );
        assert_eq!(display_width(&pad_visible(plain, 10)), 10);
        assert_eq!(display_width(&pad_visible(styled, 10)), 10);
    }

    #[test]
    fn display_width_strips_ansi_then_measures_columns() {
        assert_eq!(display_width("\x1b[1;36mhello\x1b[0m"), 5);
        assert_eq!(display_width("plain"), 5);
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn display_width_counts_wide_cjk_as_two() {
        assert_eq!(display_width("中文"), 4);
        assert_eq!(display_width("a中b"), 4);
        // A zero-width combining mark adds no columns.
        assert_eq!(display_width("e\u{0301}"), 1);
    }

    #[test]
    fn truncate_visible_does_not_overflow_on_wide_chars() {
        // Four full-width glyphs = 8 columns; budget 5 must not overflow the cell.
        let out = truncate_visible_single_line("中中中中", 5);
        assert!(
            display_width(&out) <= 5,
            "width {} exceeds 5",
            display_width(&out)
        );
        // Two glyphs (4 cols) fit; a third (2 cols) would reach 6 > 5.
        assert_eq!(display_width(&out), 4);
    }

    #[test]
    fn status_tone_maps_known_lifecycle_words() {
        assert_eq!(status_tone("completed"), Tone::Ok);
        assert_eq!(status_tone("polished"), Tone::Ok);
        assert_eq!(status_tone("running"), Tone::Heading);
        assert_eq!(status_tone("paused"), Tone::Paused);
        assert_eq!(status_tone("failed"), Tone::Negative);
        assert_eq!(status_tone("killed"), Tone::Negative);
        assert_eq!(status_tone("done criteria failed"), Tone::Negative);
        assert_eq!(status_tone("pending"), Tone::Note);
        assert_eq!(status_tone("note"), Tone::Note);
        assert_eq!(status_tone("warning"), Tone::Warn);
    }

    #[test]
    fn same_status_maps_to_same_tone_in_line_and_tui() {
        // A status word's line tone and its TUI color come from the same Tone.
        assert_eq!(
            status_tone("running").to_tui_color(),
            TUI_PALETTE.status_running
        );
        assert_eq!(
            status_tone("completed").to_tui_color(),
            TUI_PALETTE.status_completed
        );
        assert_eq!(
            status_tone("failed").to_tui_color(),
            TUI_PALETTE.status_failed
        );
    }

    #[test]
    fn unknown_status_is_explicit_not_silently_dimmed() {
        // An unrecognized status resolves to the explicit Unknown class, not a
        // silent catch-all, and renders in a visible (non-dim) default tone.
        assert_eq!(Status::classify("totally-made-up-state"), Status::Unknown);
        assert_eq!(status_tone("totally-made-up-state"), Tone::Plain);
        assert_ne!(status_tone("totally-made-up-state"), Tone::Muted);
        assert_ne!(status_tone("totally-made-up-state"), Tone::Note);
    }

    #[test]
    fn replacement_line_stays_one_column_short_of_terminal_width() {
        let line = replacement_line_text(
            "\x1b[1;36mdeadreckoning\x1b[0m plan 4b4fdc93 running; done=1/4 running=task-1:f0c66203,task-2:f63cbe05 pending=1 failed=0; attach deadreckon attach 4b4fdc93",
            40,
        );

        assert_eq!(strip_ansi(&line).chars().count(), 39);
        assert!(line.contains("\x1b[0m"));
    }

    #[test]
    fn replacement_line_resets_truncated_active_style() {
        let line = replacement_line_text("\x1b[1;36mdeadreckoning plan is still running", 18);

        assert_eq!(strip_ansi(&line).chars().count(), 17);
        assert!(line.ends_with("\x1b[0m"));
    }

    #[test]
    fn replacement_line_collapses_newlines_before_printing() {
        let line = replacement_line_text("first\nsecond\rthird", 80);

        assert_eq!(line, "first second third");
    }
}
