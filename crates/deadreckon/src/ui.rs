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
    match stream {
        Stream::Stdout => io::stdout().is_terminal(),
        Stream::Stderr => io::stderr().is_terminal(),
    }
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
    match status.as_ref().trim().to_ascii_lowercase().as_str() {
        "ok" | "ready" | "set" | "wrote" | "updated" | "installed" | "completed" | "passed"
        | "polished" | "applied" | "cleaned" | "exported" => Tone::Ok,
        "running" => Tone::Heading,
        "failed" | "killed" | "error" | "missing" | "refused" => Tone::Negative,
        "pending" | "planned" | "paused" | "skipped" | "undone" | "warning" | "warn" => Tone::Warn,
        _ => Tone::Warn,
    }
}

pub(crate) fn render_status(stream: Stream, status: impl AsRef<str>) -> String {
    let status = status.as_ref();
    render(stream, status_tone(status), status)
}

#[allow(dead_code)]
pub(crate) fn write(stream: Stream, tone: Tone, text: impl AsRef<str>) -> io::Result<()> {
    let rendered = render(stream, tone, text);
    match stream {
        Stream::Stdout => {
            print!("{rendered}");
            io::stdout().flush()
        }
        Stream::Stderr => {
            eprint!("{rendered}");
            io::stderr().flush()
        }
    }
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

#[allow(dead_code)]
pub(crate) fn kv_block<K, V>(stream: Stream, items: &[(K, V)]) -> io::Result<()>
where
    K: AsRef<str>,
    V: AsRef<str>,
{
    let width = items
        .iter()
        .map(|(key, _)| key.as_ref().chars().count())
        .max()
        .unwrap_or(0);
    for (key, value) in items {
        writeln(
            stream,
            Tone::Plain,
            format!("{:<width$}: {}", key.as_ref(), value.as_ref()),
        )?;
    }
    Ok(())
}

pub(crate) fn clear_current_line(stream: Stream) -> io::Result<()> {
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
    match stream {
        Stream::Stdout => {
            print!("\r{}\x1b[K", text.as_ref());
            io::stdout().flush()
        }
        Stream::Stderr => {
            eprint!("\r{}\x1b[K", text.as_ref());
            io::stderr().flush()
        }
    }
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
        Tone::Negative => Some("1;31"),
        Tone::Prompt => Some("1;36"),
        Tone::Hint => Some("1;34"),
    }
}

#[cfg(test)]
mod tests {
    use super::{Tone, status_tone};

    #[test]
    fn status_tone_maps_known_lifecycle_words() {
        assert_eq!(status_tone("completed"), Tone::Ok);
        assert_eq!(status_tone("polished"), Tone::Ok);
        assert_eq!(status_tone("running"), Tone::Heading);
        assert_eq!(status_tone("paused"), Tone::Warn);
        assert_eq!(status_tone("failed"), Tone::Negative);
        assert_eq!(status_tone("killed"), Tone::Negative);
        assert_eq!(status_tone("unknown"), Tone::Warn);
    }
}
