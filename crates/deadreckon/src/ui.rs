use std::io::{self, IsTerminal, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::style::Color;

static PLAIN_OUTPUT: AtomicBool = AtomicBool::new(false);

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
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
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
