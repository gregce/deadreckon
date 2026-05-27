use std::io::{self, Write as _};

use crate::Result;
use crate::ui::{self, Stream, Tone};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectChoice {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) detail: Option<String>,
}

impl SelectChoice {
    pub(crate) fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: None,
        }
    }

    pub(crate) fn with_detail(
        id: impl Into<String>,
        label: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: Some(detail.into()),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SelectPrompt {
    pub(crate) title: String,
    pub(crate) help: Option<String>,
    pub(crate) choices: Vec<SelectChoice>,
    pub(crate) default_index: usize,
}

pub(crate) fn open(message: &str, _default: Option<&str>) -> Result<String> {
    ui::write(Stream::Stdout, Tone::Prompt, "?")?;
    print!(" {message}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim_end_matches(&['\r', '\n'][..]).to_string())
}

pub(crate) fn select_one(prompt: &SelectPrompt) -> Result<SelectChoice> {
    println!("{}", prompt.title);
    if let Some(help) = prompt
        .help
        .as_deref()
        .filter(|help| !help.trim().is_empty())
    {
        println!("  {help}");
    }
    for (index, choice) in prompt.choices.iter().enumerate() {
        let ordinal = index + 1;
        match choice.detail.as_deref() {
            Some(detail) if !detail.trim().is_empty() => {
                println!("  [{ordinal}] {} - {detail}", choice.label);
            }
            _ => println!("  [{ordinal}] {}", choice.label),
        }
    }
    let default = prompt
        .default_index
        .min(prompt.choices.len().saturating_sub(1))
        + 1;
    loop {
        let answer = open(&format!("choose [{default}]: "), None)?;
        if let Some(index) = parse_select_answer(&answer, default, prompt.choices.len()) {
            return Ok(prompt.choices[index].clone());
        }
        println!("Please choose a number from 1 to {}.", prompt.choices.len());
    }
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

fn parse_select_answer(answer: &str, default: usize, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let trimmed = answer.trim();
    if trimmed.is_empty() {
        return Some(default.saturating_sub(1).min(len - 1));
    }
    let value = trimmed.parse::<usize>().ok()?;
    if (1..=len).contains(&value) {
        Some(value - 1)
    } else {
        None
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
    use super::{parse_confirm_answer, parse_select_answer};

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

    #[test]
    fn select_answer_accepts_number_and_default() {
        assert_eq!(parse_select_answer("", 2, 4), Some(1));
        assert_eq!(parse_select_answer("1", 2, 4), Some(0));
        assert_eq!(parse_select_answer("4", 2, 4), Some(3));
    }

    #[test]
    fn select_answer_rejects_out_of_range_and_text() {
        assert_eq!(parse_select_answer("0", 2, 4), None);
        assert_eq!(parse_select_answer("5", 2, 4), None);
        assert_eq!(parse_select_answer("review", 2, 4), None);
    }
}
