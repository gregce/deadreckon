use std::io::{self, Write as _};

use crate::Result;
use crate::ui::{self, Stream, Tone};

pub(crate) fn open(message: &str, _default: Option<&str>) -> Result<String> {
    ui::write(Stream::Stdout, Tone::Prompt, "?")?;
    print!(" {message}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim_end_matches(&['\r', '\n'][..]).to_string())
}

pub(crate) fn confirm(question: &str, default_yes: bool) -> Result<bool> {
    let marker = if default_yes { "[Y/n]" } else { "[y/N]" };
    let answer = open(&format!("{question} {marker}: "), None)?;
    let trimmed = answer.trim();
    if trimmed.is_empty() {
        return Ok(default_yes);
    }
    Ok(matches!(trimmed.to_ascii_lowercase().as_str(), "y" | "yes"))
}
