//! One prompt engine for every interactive surface.
//!
//! On a TTY (and unless `DEADRECKON_PROMPT_LINE_MODE` is set) prompts render
//! through `inquire` — arrow-key selects with hints, styled confirms, and
//! text inputs themed to the shared [`Tone`] palette. Off-TTY, in line mode,
//! and under `--plain`-style gating, everything falls back to the original
//! numbered line prompts so scripts and tests keep byte-stable behavior.
//! Esc cancels: a menu with a choice whose id is `"cancel"` resolves to that
//! choice; otherwise the prompt errors with `Interrupted`, which callers
//! surface as a blocked verdict.

use std::fmt;
use std::io::{self, IsTerminal as _, Write as _};

use crate::Result;
use crate::ui::{self, Stream, Tone};
use inquire::InquireError;
use inquire::ui::{Attributes, Color, RenderConfig, StyleSheet, Styled};

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

/// How a [`SelectChoice`] renders inside an inquire menu: `label — detail`.
#[derive(Clone)]
struct ChoiceItem {
    index: usize,
    text: String,
}

impl fmt::Display for ChoiceItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

fn choice_item(index: usize, choice: &SelectChoice) -> ChoiceItem {
    let text = match choice.detail.as_deref().filter(|d| !d.trim().is_empty()) {
        Some(detail) => format!("{} — {detail}", choice.label),
        None => choice.label.clone(),
    };
    ChoiceItem { index, text }
}

/// The shared inquire theme, derived from the [`Tone`] palette: cyan prompt
/// marker and selection, dim help text. Colorless when the stdout color gate
/// (`--plain`, `NO_COLOR`, dumb terminal) is off.
fn render_config() -> RenderConfig<'static> {
    if !ui::enabled(Stream::Stdout) {
        return RenderConfig::empty();
    }
    RenderConfig::default_colored()
        .with_prompt_prefix(Styled::new("?").with_fg(Color::LightCyan))
        .with_answered_prompt_prefix(Styled::new("✓").with_fg(Color::LightGreen))
        .with_highlighted_option_prefix(Styled::new("›").with_fg(Color::LightCyan))
        .with_selected_option(Some(
            StyleSheet::new()
                .with_fg(Color::LightCyan)
                .with_attr(Attributes::BOLD),
        ))
        .with_help_message(StyleSheet::new().with_fg(Color::DarkGrey))
        .with_answer(StyleSheet::new().with_fg(Color::LightCyan))
}

/// Whether prompts render interactively (inquire) rather than as numbered
/// line prompts — callers can branch when the two modes need different copy.
pub(crate) fn is_interactive() -> bool {
    interactive()
}

/// TTY presence regardless of line mode: rescue prompts render through
/// `select_one`, which handles both inquire and numbered-line rendering, so
/// the gate is only "is a human attached".
pub(crate) fn is_tty() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn interactive() -> bool {
    io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && std::env::var_os("DEADRECKON_PROMPT_LINE_MODE").is_none()
}

fn cancelled() -> crate::CliError {
    io::Error::new(io::ErrorKind::Interrupted, "prompt cancelled").into()
}

pub(crate) fn open(message: &str, _default: Option<&str>) -> Result<String> {
    if interactive() {
        let trimmed = message.trim_end().trim_end_matches(':').trim_end();
        return match inquire::Text::new(trimmed)
            .with_render_config(render_config())
            .prompt()
        {
            Ok(value) => Ok(value.trim().to_string()),
            Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
                Err(cancelled())
            }
            Err(err) => Err(io::Error::other(err.to_string()).into()),
        };
    }
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
    if interactive() {
        return select_one_interactive(prompt);
    }
    select_one_line(prompt)
}

fn select_one_interactive(prompt: &SelectPrompt) -> Result<SelectChoice> {
    let items = prompt
        .choices
        .iter()
        .enumerate()
        .map(|(index, choice)| choice_item(index, choice))
        .collect::<Vec<_>>();
    let default_index = prompt
        .default_index
        .min(prompt.choices.len().saturating_sub(1));
    let mut select = inquire::Select::new(&prompt.title, items)
        .with_render_config(render_config())
        .with_starting_cursor(default_index)
        .with_page_size(12);
    if let Some(help) = prompt
        .help
        .as_deref()
        .filter(|help| !help.trim().is_empty())
    {
        select = select.with_help_message(help);
    }
    match select.prompt() {
        Ok(item) => Ok(prompt.choices[item.index].clone()),
        Err(InquireError::OperationCanceled) => match cancel_choice_index(prompt) {
            Some(index) => Ok(prompt.choices[index].clone()),
            None => Err(cancelled()),
        },
        Err(InquireError::OperationInterrupted) => Err(cancelled()),
        Err(err) => Err(io::Error::other(err.to_string()).into()),
    }
}

fn select_one_line(prompt: &SelectPrompt) -> Result<SelectChoice> {
    print_select_header(prompt);
    for (index, choice) in prompt.choices.iter().enumerate() {
        let ordinal = index + 1;
        let number = ui::render(Stream::Stdout, Tone::Command, format!("[{ordinal}]"));
        match choice.detail.as_deref() {
            Some(detail) if !detail.trim().is_empty() => {
                println!(
                    "  {number} {} - {}",
                    choice.label,
                    ui::render(Stream::Stdout, Tone::Muted, detail)
                );
            }
            _ => println!("  {number} {}", choice.label),
        }
    }
    let default = prompt
        .default_index
        .min(prompt.choices.len().saturating_sub(1))
        + 1;
    loop {
        let answer = open_line(&format!("choose [{default}]: "))?;
        if let Some(index) = parse_select_answer(&answer, default, prompt.choices.len()) {
            return Ok(prompt.choices[index].clone());
        }
        println!("Please choose a number from 1 to {}.", prompt.choices.len());
    }
}

/// Plain line input regardless of TTY: used inside line-mode loops where the
/// numbered header has already been printed.
fn open_line(message: &str) -> Result<String> {
    print!("{}", ui::render(Stream::Stdout, Tone::Prompt, "?"));
    print!(" {message}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim_end_matches(&['\r', '\n'][..]).to_string())
}

fn print_select_header(prompt: &SelectPrompt) {
    println!(
        "{}",
        ui::render(Stream::Stdout, Tone::Heading, &prompt.title)
    );
    if let Some(help) = prompt
        .help
        .as_deref()
        .filter(|help| !help.trim().is_empty())
    {
        println!("  {}", ui::render(Stream::Stdout, Tone::Muted, help));
    }
}

fn cancel_choice_index(prompt: &SelectPrompt) -> Option<usize> {
    prompt
        .choices
        .iter()
        .position(|choice| choice.id == "cancel")
}

pub(crate) fn confirm(question: &str, default_yes: bool) -> Result<bool> {
    if interactive() {
        return match inquire::Confirm::new(question)
            .with_render_config(render_config())
            .with_default(default_yes)
            .prompt()
        {
            Ok(value) => Ok(value),
            Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
                Err(cancelled())
            }
            Err(err) => Err(io::Error::other(err.to_string()).into()),
        };
    }
    let marker = if default_yes { "[Y/n]" } else { "[y/N]" };
    loop {
        let answer = open_line(&format!("{question} {marker}: "))?;
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

/// Prompt for a number in `range`, re-prompting (never erroring) on empty input
/// (which takes `default`), non-numeric input, or an out-of-range value. Use this
/// for count-style prompts instead of `open(...).parse()?`, which aborts the whole
/// command on a typo.
pub(crate) fn ask_number(
    label: &str,
    range: std::ops::RangeInclusive<usize>,
    default: usize,
) -> Result<usize> {
    if interactive() {
        let start = *range.start();
        let end = *range.end();
        return match inquire::CustomType::<usize>::new(label)
            .with_render_config(render_config())
            .with_default(default)
            .with_help_message(&format!("{start} to {end}"))
            .with_validator(move |value: &usize| {
                if (start..=end).contains(value) {
                    Ok(inquire::validator::Validation::Valid)
                } else {
                    Ok(inquire::validator::Validation::Invalid(
                        format!("enter a number from {start} to {end}").into(),
                    ))
                }
            })
            .prompt()
        {
            Ok(value) => Ok(value),
            Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
                Err(cancelled())
            }
            Err(err) => Err(io::Error::other(err.to_string()).into()),
        };
    }
    loop {
        let answer = open_line(&format!("{label} [{default}]: "))?;
        if let Some(value) = parse_number_in_range(&answer, &range, default) {
            return Ok(value);
        }
        println!(
            "Please enter a number from {} to {}.",
            range.start(),
            range.end()
        );
    }
}

fn parse_number_in_range(
    answer: &str,
    range: &std::ops::RangeInclusive<usize>,
    default: usize,
) -> Option<usize> {
    let trimmed = answer.trim();
    if trimmed.is_empty() {
        return Some(default);
    }
    let value = trimmed.parse::<usize>().ok()?;
    if range.contains(&value) {
        Some(value)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SelectChoice, SelectPrompt, cancel_choice_index, choice_item, parse_confirm_answer,
        parse_number_in_range, parse_select_answer,
    };

    fn prompt_with(choices: Vec<SelectChoice>) -> SelectPrompt {
        SelectPrompt {
            title: "Choose".to_string(),
            help: None,
            choices,
            default_index: 0,
        }
    }

    #[test]
    fn choice_items_render_label_with_dimmable_detail() {
        let plain = choice_item(0, &SelectChoice::new("run", "New run"));
        assert_eq!(plain.text, "New run");
        let detailed = choice_item(
            1,
            &SelectChoice::with_detail("run", "New run", "equivalent to --mode run"),
        );
        assert_eq!(detailed.text, "New run — equivalent to --mode run");
        // Whitespace-only detail renders like no detail at all.
        let blank = choice_item(2, &SelectChoice::with_detail("run", "New run", "  "));
        assert_eq!(blank.text, "New run");
    }

    #[test]
    fn esc_maps_to_the_cancel_choice_when_present() {
        let with_cancel = prompt_with(vec![
            SelectChoice::new("run", "New run"),
            SelectChoice::new("cancel", "Cancel"),
        ]);
        assert_eq!(cancel_choice_index(&with_cancel), Some(1));
        let without = prompt_with(vec![SelectChoice::new("run", "New run")]);
        assert_eq!(cancel_choice_index(&without), None);
    }

    #[test]
    fn line_mode_select_parsing_accepts_default_number_and_rejects_garbage() {
        assert_eq!(parse_select_answer("", 2, 3), Some(1));
        assert_eq!(parse_select_answer("3", 1, 3), Some(2));
        assert_eq!(parse_select_answer("0", 1, 3), None);
        assert_eq!(parse_select_answer("4", 1, 3), None);
        assert_eq!(parse_select_answer("x", 1, 3), None);
        assert_eq!(parse_select_answer("", 1, 0), None);
    }

    #[test]
    fn confirm_parsing_accepts_y_n_yes_no_and_default() {
        assert_eq!(parse_confirm_answer("", true), Some(true));
        assert_eq!(parse_confirm_answer("", false), Some(false));
        assert_eq!(parse_confirm_answer("y", false), Some(true));
        assert_eq!(parse_confirm_answer("YES", false), Some(true));
        assert_eq!(parse_confirm_answer("n", true), Some(false));
        assert_eq!(parse_confirm_answer("maybe", true), None);
    }

    #[test]
    fn number_parsing_defaults_and_bounds() {
        let range = 2..=6;
        assert_eq!(parse_number_in_range("", &range, 3), Some(3));
        assert_eq!(parse_number_in_range("4", &range, 3), Some(4));
        assert_eq!(parse_number_in_range("7", &range, 3), None);
        assert_eq!(parse_number_in_range("one", &range, 3), None);
    }
}
