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
pub(crate) enum Tone {
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
    status_running: Color::Cyan,
    status_completed: Color::Green,
    status_failed: Color::Red,
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
    let Some(code) = tone_code(tone) else {
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

pub(crate) fn status_tone(status: impl AsRef<str>) -> Tone {
    let status = status.as_ref().trim().to_ascii_lowercase();
    if status.contains("failed")
        || status.contains("killed")
        || status.contains("error")
        || status.contains("missing")
        || status.contains("refused")
    {
        return Tone::Negative;
    }
    if status.contains("paused") {
        return Tone::Paused;
    }
    if status.contains("warning") || status.contains("warn") {
        return Tone::Warn;
    }
    match status.as_str() {
        "ok" | "ready" | "set" | "wrote" | "updated" | "installed" | "completed" | "passed"
        | "polished" | "applied" | "cleaned" | "exported" => Tone::Ok,
        "running" => Tone::Heading,
        "pending" | "planned" | "skipped" | "undone" | "recorded" | "note" => Tone::Note,
        _ => Tone::Note,
    }
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

pub(crate) fn ui_note(text: impl AsRef<str>) -> String {
    render(Stream::Stdout, Tone::Note, text)
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
        if visible >= max_visible {
            break;
        }
        match ch {
            '\n' | '\r' => out.push(' '),
            _ => out.push(ch),
        }
        visible += 1;
    }
    if style_active {
        out.push_str(ANSI_RESET);
    }
    out
}

fn tone_code(tone: Tone) -> Option<&'static str> {
    match tone {
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

#[cfg(test)]
mod tests {
    use super::{Tone, replacement_line_text, status_tone, strip_ansi};

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
        assert_eq!(status_tone("unknown"), Tone::Note);
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
