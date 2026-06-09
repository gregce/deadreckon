use std::io::{self, IsTerminal as _, Write as _};

use crate::Result;
use crate::ui::{self, Stream, Tone};
use crossterm::cursor::{MoveToColumn, MoveUp};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
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
        let answer = open(&format!("choose [{default}]: "), None)?;
        if let Some(index) = parse_select_answer(&answer, default, prompt.choices.len()) {
            return Ok(prompt.choices[index].clone());
        }
        println!("Please choose a number from 1 to {}.", prompt.choices.len());
    }
}

fn select_one_menu(prompt: &SelectPrompt) -> Result<SelectChoice> {
    // A list taller than the terminal can't be redrawn in place; use line mode.
    if !menu_fits(prompt.choices.len(), terminal_rows()) {
        return select_one_line(prompt);
    }
    print_select_header(prompt);
    let mut selected = prompt
        .default_index
        .min(prompt.choices.len().saturating_sub(1));
    let cancel_index = cancel_choice_index(prompt);
    let mut buffer = String::new();
    let _raw_mode = RawModeGuard::enable()?;
    render_select_menu(prompt, selected, false, false)?;
    loop {
        let Event::Key(key) = event::read()? else {
            continue;
        };
        match menu_step(
            prompt.choices.len(),
            selected,
            cancel_index,
            &mut buffer,
            key,
        ) {
            MenuStep::Move(new_selected) => {
                selected = new_selected;
                buffer.clear();
                render_select_menu(prompt, selected, true, false)?;
            }
            MenuStep::Commit(index) => {
                finish_select_menu_line()?;
                return Ok(prompt.choices[index].clone());
            }
            MenuStep::Accumulate => {
                render_select_menu(prompt, selected, true, false)?;
            }
            MenuStep::Reject => {
                render_select_menu(prompt, selected, true, true)?;
            }
            MenuStep::Cancel | MenuStep::Interrupt => {
                finish_select_menu_line()?;
                return Err(io::Error::new(io::ErrorKind::Interrupted, "prompt cancelled").into());
            }
            MenuStep::Ignore => {}
        }
    }
}

fn render_select_menu(
    prompt: &SelectPrompt,
    selected: usize,
    redraw: bool,
    out_of_range: bool,
) -> Result<()> {
    let mut stdout = io::stdout();
    if redraw {
        execute!(stdout, MoveUp(prompt.choices.len() as u16), MoveToColumn(0))?;
    }
    let width = selectable_menu_width();
    for (index, choice) in prompt.choices.iter().enumerate() {
        let ordinal = index + 1;
        let selected_row = index == selected;
        let marker = if selected_row { ">" } else { " " };
        execute!(stdout, Clear(ClearType::CurrentLine))?;
        let line = match choice.detail.as_deref() {
            Some(detail) if !detail.trim().is_empty() => {
                format!("  {marker} [{ordinal}] {} - {detail}", choice.label)
            }
            _ => format!("  {marker} [{ordinal}] {}", choice.label),
        };
        let line = style_menu_row(&truncate_menu_line(&line, width), ordinal, selected_row);
        write_select_menu_line(&mut stdout, &line)?;
    }
    execute!(stdout, Clear(ClearType::CurrentLine))?;
    let default = prompt
        .default_index
        .min(prompt.choices.len().saturating_sub(1))
        + 1;
    let notice = if out_of_range {
        format!(" — choose 1-{}", prompt.choices.len())
    } else {
        String::new()
    };
    write!(
        stdout,
        "{} {} ",
        ui::render(Stream::Stdout, Tone::Prompt, "?"),
        ui::render(
            Stream::Stdout,
            Tone::Muted,
            format!("choose [{default}]: arrows/Enter, number, Esc to cancel{notice}"),
        )
    )?;
    stdout.flush()?;
    Ok(())
}

fn write_select_menu_line(stdout: &mut impl io::Write, line: &str) -> io::Result<()> {
    write!(stdout, "{line}\r\n")
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

/// Colorize a normalized, already-truncated select-menu row. Color is applied
/// only on a TTY (ui::render is gated) and only after width truncation, so the
/// visible width is unchanged and the redraw (MoveUp by line count) math holds.
fn style_menu_row(line: &str, ordinal: usize, selected: bool) -> String {
    if !ui::enabled(Stream::Stdout) {
        return line.to_string();
    }
    paint_menu_row(line, ordinal, selected, |tone, text| {
        ui::render(Stream::Stdout, tone, text)
    })
}

/// Pure painter for a select-menu row: splits the line into `lead | [n] | body`
/// and applies `paint` to the marker, ordinal, and (for the selected row) the
/// label, dimming the trailing detail. `paint` only ever wraps tokens, so the
/// visible width equals the input — stripping the result yields `line` again.
fn paint_menu_row(
    line: &str,
    ordinal: usize,
    selected: bool,
    paint: impl Fn(Tone, &str) -> String,
) -> String {
    let token = format!("[{ordinal}]");
    let Some(at) = line.find(&token) else {
        return line.to_string();
    };
    let lead = &line[..at];
    let rest = &line[at + token.len()..];
    let lead = if selected {
        lead.replacen('>', &paint(Tone::Prompt, ">"), 1)
    } else {
        lead.to_string()
    };
    let number = paint(Tone::Command, &token);
    let body = match rest.split_once(" - ") {
        Some((label, detail)) => {
            let label = if selected {
                paint(Tone::Heading, label)
            } else {
                label.to_string()
            };
            format!("{label} - {}", paint(Tone::Muted, detail))
        }
        None if selected => paint(Tone::Heading, rest),
        None => rest.to_string(),
    };
    format!("{lead}{number}{body}")
}

fn selectable_menu_width() -> usize {
    crossterm::terminal::size()
        .ok()
        .map(|(columns, _)| usize::from(columns))
        .or_else(|| {
            std::env::var("COLUMNS")
                .ok()
                .and_then(|columns| columns.parse::<usize>().ok())
        })
        .filter(|columns| *columns > 1)
        .map(|columns| columns.saturating_sub(1))
        .unwrap_or(119)
}

fn truncate_menu_line(line: &str, max_visible: usize) -> String {
    let normalized = normalize_menu_line(line);
    if normalized.chars().count() <= max_visible {
        return normalized;
    }
    if max_visible == 0 {
        return String::new();
    }
    if max_visible <= 3 {
        return normalized.chars().take(max_visible).collect();
    }
    let keep = max_visible.saturating_sub(3);
    let mut out = normalized.chars().take(keep).collect::<String>();
    out.push_str("...");
    out
}

fn normalize_menu_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut previous_was_space = false;
    for ch in line.chars() {
        if ch.is_whitespace() {
            if !previous_was_space {
                out.push(' ');
                previous_was_space = true;
            }
        } else {
            out.push(ch);
            previous_was_space = false;
        }
    }
    out.trim_end().to_string()
}

fn finish_select_menu_line() -> io::Result<()> {
    let mut stdout = io::stdout();
    write!(stdout, "\r\n")?;
    stdout.flush()
}

#[derive(Debug, PartialEq, Eq)]
enum DigitResolution {
    Commit(usize),
    Accumulate,
    OutOfRange,
}

/// Resolve an accumulated digit `buffer` against a menu of `len` choices. Commits
/// as soon as no longer prefix could be valid, so single-digit menus feel instant
/// while menus with 10+ choices accept multi-digit entry.
fn resolve_digit_selection(buffer: &str, len: usize) -> DigitResolution {
    let Ok(value) = buffer.parse::<usize>() else {
        return DigitResolution::OutOfRange;
    };
    if value == 0 || value > len {
        return DigitResolution::OutOfRange;
    }
    if value.saturating_mul(10) > len {
        DigitResolution::Commit(value - 1)
    } else {
        DigitResolution::Accumulate
    }
}

#[derive(Debug, PartialEq, Eq)]
enum MenuStep {
    Move(usize),
    Commit(usize),
    Accumulate,
    Reject,
    Cancel,
    Interrupt,
    Ignore,
}

/// Pure key-dispatch for the selectable menu, factored out so the keyboard and
/// number-input behavior is unit-testable without a real TTY. `buffer` holds the
/// in-progress multi-digit selection and is mutated as digits arrive.
fn menu_step(
    choice_count: usize,
    selected: usize,
    cancel_index: Option<usize>,
    buffer: &mut String,
    key: KeyEvent,
) -> MenuStep {
    if matches!(key.kind, KeyEventKind::Release) {
        return MenuStep::Ignore;
    }
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => MenuStep::Interrupt,
        KeyCode::Char('j' | 'm') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            MenuStep::Commit(selected)
        }
        KeyCode::Enter => {
            if !buffer.is_empty()
                && let Ok(value) = buffer.parse::<usize>()
                && (1..=choice_count).contains(&value)
            {
                return MenuStep::Commit(value - 1);
            }
            MenuStep::Commit(selected)
        }
        KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
            MenuStep::Move(selected.saturating_sub(1))
        }
        KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
            MenuStep::Move((selected + 1).min(choice_count.saturating_sub(1)))
        }
        KeyCode::Home => MenuStep::Move(0),
        KeyCode::End => MenuStep::Move(choice_count.saturating_sub(1)),
        KeyCode::Char(value) if key.modifiers.is_empty() && value.is_ascii_digit() => {
            buffer.push(value);
            match resolve_digit_selection(buffer, choice_count) {
                DigitResolution::Commit(index) => MenuStep::Commit(index),
                DigitResolution::Accumulate => MenuStep::Accumulate,
                DigitResolution::OutOfRange => {
                    buffer.clear();
                    MenuStep::Reject
                }
            }
        }
        KeyCode::Esc => match cancel_index {
            Some(index) => MenuStep::Commit(index),
            None => MenuStep::Cancel,
        },
        _ => MenuStep::Ignore,
    }
}

fn cancel_choice_index(prompt: &SelectPrompt) -> Option<usize> {
    prompt
        .choices
        .iter()
        .position(|choice| choice.id == "cancel")
}

/// Whether a menu of `choice_count` choices fits in a terminal `rows` tall. Tall
/// menus fall back to line mode because the raw-mode redraw can only MoveUp
/// within the visible region (a taller list corrupts the screen).
fn menu_fits(choice_count: usize, rows: usize) -> bool {
    // title + optional help + the prompt line + a little slack.
    choice_count + 4 <= rows
}

fn terminal_rows() -> usize {
    crossterm::terminal::size()
        .ok()
        .map(|(_, rows)| usize::from(rows))
        .filter(|rows| *rows > 0)
        .unwrap_or(24)
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

/// Prompt for a number in `range`, re-prompting (never erroring) on empty input
/// (which takes `default`), non-numeric input, or an out-of-range value. Use this
/// for count-style prompts instead of `open(...).parse()?`, which aborts the whole
/// command on a typo.
pub(crate) fn ask_number(
    label: &str,
    range: std::ops::RangeInclusive<usize>,
    default: usize,
) -> Result<usize> {
    loop {
        let answer = open(&format!("{label} [{default}]: "), None)?;
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
        MenuStep, menu_fits, menu_step, paint_menu_row, parse_confirm_answer, parse_select_answer,
        style_menu_row, truncate_menu_line, write_select_menu_line,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(value: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(value), KeyModifiers::empty())
    }

    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::empty())
    }

    #[test]
    fn menu_mode_selects_choice_above_nine_by_number() {
        // A 12-choice menu accepts multi-digit entry: "1" then "2" commits choice 12.
        let mut buffer = String::new();
        assert_eq!(
            menu_step(12, 0, None, &mut buffer, key('1')),
            MenuStep::Accumulate
        );
        assert_eq!(
            menu_step(12, 0, None, &mut buffer, key('2')),
            MenuStep::Commit(11)
        );
        // "1" then "0" commits choice 10 (index 9).
        let mut buffer = String::new();
        assert_eq!(
            menu_step(12, 0, None, &mut buffer, key('1')),
            MenuStep::Accumulate
        );
        assert_eq!(
            menu_step(12, 0, None, &mut buffer, key('0')),
            MenuStep::Commit(9)
        );
    }

    #[test]
    fn esc_cancels_without_explicit_cancel_choice() {
        let mut buffer = String::new();
        // No explicit cancel choice: Esc cancels gracefully anyway.
        assert_eq!(menu_step(4, 0, None, &mut buffer, esc()), MenuStep::Cancel);
        // With an explicit cancel choice: Esc selects it.
        assert_eq!(
            menu_step(4, 0, Some(3), &mut buffer, esc()),
            MenuStep::Commit(3)
        );
    }

    #[test]
    fn menu_mode_reports_out_of_range() {
        let mut buffer = String::new();
        // 4 choices: "9" is out of range, so the buffer clears and the menu reports it.
        assert_eq!(
            menu_step(4, 0, None, &mut buffer, key('9')),
            MenuStep::Reject
        );
        assert!(buffer.is_empty());
        assert_eq!(
            menu_step(4, 0, None, &mut buffer, key('0')),
            MenuStep::Reject
        );
    }

    #[test]
    fn tall_menu_falls_back_or_paginates() {
        assert!(menu_fits(5, 24), "a short menu fits");
        assert!(!menu_fits(30, 24), "a 30-choice menu must fall back");
        assert!(!menu_fits(21, 24), "21 choices + chrome exceeds 24 rows");
    }

    #[test]
    fn ask_number_reprompts_on_non_numeric_and_out_of_range() {
        let range = 2..=6;
        assert_eq!(super::parse_number_in_range("", &range, 3), Some(3));
        assert_eq!(super::parse_number_in_range("4", &range, 3), Some(4));
        assert_eq!(super::parse_number_in_range("9", &range, 3), None);
        assert_eq!(super::parse_number_in_range("0", &range, 3), None);
        assert_eq!(super::parse_number_in_range("x", &range, 3), None);
    }

    #[test]
    fn campaign_count_prompt_loops_instead_of_exiting() {
        // The campaign/orchestrate count prompts route through ask_number (2..=6):
        // bad input returns None so the prompt re-prompts, where the old
        // parse::<u8>()? aborted the whole command.
        let range = 2..=6;
        assert_eq!(
            super::parse_number_in_range("not-a-number", &range, 4),
            None
        );
        assert_eq!(super::parse_number_in_range("99", &range, 4), None);
        assert_eq!(super::parse_number_in_range("3", &range, 4), Some(3));
    }

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
    fn selectable_menu_lines_return_to_column_zero_in_raw_mode() {
        let mut output = Vec::new();
        write_select_menu_line(&mut output, "  > [1] Run").expect("write line");
        assert_eq!(output, b"  > [1] Run\r\n");
    }

    #[test]
    fn selectable_menu_lines_are_single_terminal_rows() {
        let line = truncate_menu_line(
            "  > [1] Recommended: campaign orchestration - a very long detail",
            32,
        );

        assert_eq!(line.chars().count(), 32);
        assert_eq!(line, " > [1] Recommended: campaign ...");
    }

    #[test]
    fn selectable_menu_lines_collapse_embedded_whitespace() {
        let line = truncate_menu_line("  > [1] Follow up\nfrom\tprevious", 80);

        assert_eq!(line, " > [1] Follow up from previous");
    }

    #[test]
    fn style_menu_row_is_plain_without_a_tty() {
        // Tests capture stdout (non-TTY), so styling must be a no-op: this is the
        // golden-safety guarantee that color never leaks into redirected output.
        assert_eq!(style_menu_row(" > [1] Run", 1, true), " > [1] Run");
        assert_eq!(
            style_menu_row(" [2] New run - equivalent to --mode run", 2, false),
            " [2] New run - equivalent to --mode run"
        );
    }

    #[test]
    fn paint_menu_row_preserves_visible_width() {
        let force = |_: crate::ui::Tone, text: &str| {
            let esc = char::from(27);
            format!("{esc}[31m{text}{esc}[0m")
        };
        for (line, ordinal, selected) in [
            (
                " > [1] Recommended: run - fallback suggested single",
                1,
                true,
            ),
            (" [2] New single supervised run - --mode run", 2, false),
            (" > [3] Cancel", 3, true),
            (" [10] Tenth option - tenth detail", 10, false),
        ] {
            let painted = paint_menu_row(line, ordinal, selected, force);
            assert!(painted.contains('\x1b'), "expected ANSI in {painted:?}");
            assert_eq!(
                crate::ui::strip_ansi(&painted),
                line,
                "visible text must be unchanged for {line:?}"
            );
        }
    }

    #[test]
    fn paint_menu_row_without_token_is_unchanged() {
        let force = |_: crate::ui::Tone, text: &str| {
            let esc = char::from(27);
            format!("{esc}[31m{text}{esc}[0m")
        };
        // An over-truncated row whose ordinal token was cut is left alone.
        assert_eq!(paint_menu_row(" > [1", 1, true, force), " > [1");
    }
}
