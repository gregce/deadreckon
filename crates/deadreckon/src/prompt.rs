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
    loop {
        let answer = open(&format!("{question} {marker}: "), None)?;
        if let Some(value) = parse_confirm_answer(&answer, default_yes) {
            return Ok(value);
        }
        println!("Please answer y or n.");
    }
}

fn parse_confirm_answer(answer: &str, default_yes: bool) -> Option<bool> {
    let trimmed = answer.trim();
    if trimmed.is_empty() {
        return Some(default_yes);
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "y" | "yes" => Some(true),
        "n" | "no" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_confirm_answer;

    #[test]
    fn confirm_answer_accepts_yes_no_and_default() {
        assert_eq!(parse_confirm_answer("", true), Some(true));
        assert_eq!(parse_confirm_answer("", false), Some(false));
        assert_eq!(parse_confirm_answer("y", false), Some(true));
        assert_eq!(parse_confirm_answer("YES", false), Some(true));
        assert_eq!(parse_confirm_answer("n", true), Some(false));
        assert_eq!(parse_confirm_answer("No", true), Some(false));
    }

    #[test]
    fn confirm_answer_rejects_free_text() {
        assert_eq!(parse_confirm_answer("README must exist", true), None);
    }
}
