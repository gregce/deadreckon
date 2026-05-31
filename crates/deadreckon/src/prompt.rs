use std::io::{self, IsTerminal as _, Write as _};

use crate::Result;
use crate::ui::{self, Stream, Tone};
use crossterm::cursor::{MoveToColumn, MoveUp};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, Clear, ClearType};

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
    print!("{}", ui::render(Stream::Stdout, Tone::Prompt, "?"));
    print!(" {message}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim_end_matches(&['\r', '\n'][..]).to_string())
}

pub(crate) fn select_one(prompt: &SelectPrompt) -> Result<SelectChoice> {
    if prompt.choices.is_empty() {
        return Err(
            io::Error::new(io::ErrorKind::InvalidInput, "select prompt has no choices").into(),
        );
    }
    if should_use_selectable_menu() {
        return select_one_menu(prompt);
    }
    select_one_line(prompt)
}

fn should_use_selectable_menu() -> bool {
    io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && std::env::var_os("DEADRECKON_PROMPT_LINE_MODE").is_none()
}

fn select_one_line(prompt: &SelectPrompt) -> Result<SelectChoice> {
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

fn select_one_menu(prompt: &SelectPrompt) -> Result<SelectChoice> {
    println!("{}", prompt.title);
    if let Some(help) = prompt
        .help
        .as_deref()
        .filter(|help| !help.trim().is_empty())
    {
        println!("  {help}");
    }
    let mut selected = prompt
        .default_index
        .min(prompt.choices.len().saturating_sub(1));
    let _raw_mode = RawModeGuard::enable()?;
    render_select_menu(prompt, selected, false)?;
    loop {
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if matches!(key.kind, KeyEventKind::Release) {
            continue;
        }
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                finish_select_menu_line()?;
                return Err(io::Error::new(io::ErrorKind::Interrupted, "prompt cancelled").into());
            }
            KeyCode::Enter => {
                finish_select_menu_line()?;
                return Ok(prompt.choices[selected].clone());
            }
            KeyCode::Char('j' | 'm') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                finish_select_menu_line()?;
                return Ok(prompt.choices[selected].clone());
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                selected = selected.saturating_sub(1);
                render_select_menu(prompt, selected, true)?;
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                if selected + 1 < prompt.choices.len() {
                    selected += 1;
                }
                render_select_menu(prompt, selected, true)?;
            }
            KeyCode::Home => {
                selected = 0;
                render_select_menu(prompt, selected, true)?;
            }
            KeyCode::End => {
                selected = prompt.choices.len().saturating_sub(1);
                render_select_menu(prompt, selected, true)?;
            }
            KeyCode::Char(value) if key.modifiers.is_empty() && value.is_ascii_digit() => {
                if let Some(index) = select_index_from_digit(value, prompt.choices.len()) {
                    selected = index;
                    render_select_menu(prompt, selected, true)?;
                }
            }
            KeyCode::Esc => {
                if let Some(index) = prompt
                    .choices
                    .iter()
                    .position(|choice| choice.id == "cancel")
                {
                    finish_select_menu_line()?;
                    return Ok(prompt.choices[index].clone());
                }
            }
            _ => {}
        }
    }
}

fn render_select_menu(prompt: &SelectPrompt, selected: usize, redraw: bool) -> Result<()> {
    let mut stdout = io::stdout();
    if redraw {
        execute!(stdout, MoveUp(prompt.choices.len() as u16), MoveToColumn(0))?;
    }
    for (index, choice) in prompt.choices.iter().enumerate() {
        let ordinal = index + 1;
        let marker = if index == selected { ">" } else { " " };
        execute!(stdout, Clear(ClearType::CurrentLine))?;
        match choice.detail.as_deref() {
            Some(detail) if !detail.trim().is_empty() => {
                write_select_menu_line(
                    &mut stdout,
                    &format!("  {marker} [{ordinal}] {} - {detail}", choice.label),
                )?;
            }
            _ => write_select_menu_line(
                &mut stdout,
                &format!("  {marker} [{ordinal}] {}", choice.label),
            )?,
        }
    }
    execute!(stdout, Clear(ClearType::CurrentLine))?;
    let default = prompt
        .default_index
        .min(prompt.choices.len().saturating_sub(1))
        + 1;
    write!(stdout, "? choose [{default}]: arrows/Enter or number ")?;
    stdout.flush()?;
    Ok(())
}

fn write_select_menu_line(stdout: &mut impl io::Write, line: &str) -> io::Result<()> {
    write!(stdout, "{line}\r\n")
}

fn finish_select_menu_line() -> io::Result<()> {
    let mut stdout = io::stdout();
    write!(stdout, "\r\n")?;
    stdout.flush()
}

fn select_index_from_digit(value: char, len: usize) -> Option<usize> {
    let digit = value.to_digit(10)? as usize;
    if (1..=len).contains(&digit) {
        Some(digit - 1)
    } else {
        None
    }
}

struct RawModeGuard;

impl RawModeGuard {
    fn enable() -> Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
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
    use super::{
        parse_confirm_answer, parse_select_answer, select_index_from_digit, write_select_menu_line,
    };

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

    #[test]
    fn selectable_menu_digit_shortcuts_match_numbered_rows() {
        assert_eq!(select_index_from_digit('1', 4), Some(0));
        assert_eq!(select_index_from_digit('4', 4), Some(3));
        assert_eq!(select_index_from_digit('5', 4), None);
        assert_eq!(select_index_from_digit('0', 4), None);
    }

    #[test]
    fn selectable_menu_lines_return_to_column_zero_in_raw_mode() {
        let mut output = Vec::new();
        write_select_menu_line(&mut output, "  > [1] Run").expect("write line");
        assert_eq!(output, b"  > [1] Run\r\n");
    }
}
