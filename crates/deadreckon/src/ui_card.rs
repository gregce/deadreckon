const DEFAULT_CARD_WIDTH: usize = 80;
const MIN_CARD_WIDTH: usize = 62;
const NARROW_FALLBACK_WIDTH: usize = 40;
const LABEL_WIDTH: usize = 14;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub title: TitleLine,
    pub subtitle: Option<String>,
    pub sections: Vec<Section>,
    pub primary_action: Option<HintLine>,
    pub hints: Vec<HintLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TitleLine {
    pub glyph: TitleGlyph,
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleGlyph {
    Success,
    Stopped,
    Paused,
    Failed,
    Preview,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Section {
    Metric {
        label: String,
        columns: Vec<MetricColumn>,
    },
    KeyValue {
        rows: Vec<(String, String)>,
    },
    Lines {
        lines: Vec<String>,
    },
    Command {
        label: String,
        command: String,
    },
    Blank,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetricColumn {
    pub value: String,
    pub tone: Tone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Good,
    Warn,
    Bad,
    Dim,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HintLine {
    pub label: String,
    pub command: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CardOptions {
    pub color: bool,
    pub plain: bool,
    pub terminal_columns: Option<usize>,
    pub no_color_env: bool,
}

pub fn render_card(card: &Card, opts: &CardOptions) -> String {
    let plain = opts.plain || opts.no_color_env;
    let color = opts.color && !plain && !opts.no_color_env;
    let mut lines = body_lines(card, plain, color);
    let terminal_width = opts.terminal_columns.unwrap_or(DEFAULT_CARD_WIDTH);
    if terminal_width < NARROW_FALLBACK_WIDTH {
        return render_narrow(&lines, plain);
    }

    let content_width = lines
        .iter()
        .map(|line| visible_length(line))
        .max()
        .unwrap_or(0);
    let desired_width = (content_width + 4).max(MIN_CARD_WIDTH);
    let card_width = desired_width.min(terminal_width.max(4));
    let inner_width = card_width.saturating_sub(4);
    let mut rendered = Vec::with_capacity(lines.len() + 2);
    rendered.push(border("top", card_width, plain, color));
    for line in lines.drain(..) {
        let content = truncate_visible_for_mode(&line, inner_width, plain);
        rendered.push(format!(
            "{}{}{}",
            style_border(if plain { "| " } else { "│ " }, color),
            pad_visible(&content, inner_width),
            style_border(if plain { " |" } else { " │" }, color)
        ));
    }
    rendered.push(border("bottom", card_width, plain, color));
    let mut text = rendered.join("\n");
    text.push('\n');
    text
}

pub fn visible_length(text: &str) -> usize {
    crate::ui::display_width(text)
}

pub fn truncate_visible(text: &str, width: usize) -> String {
    truncate_visible_inner(text, width, "…")
}

pub fn pad_visible(text: &str, width: usize) -> String {
    let len = visible_length(text);
    if len >= width {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(width - len))
    }
}

pub fn strip_ansi(text: &str) -> String {
    crate::ui::strip_ansi(text)
}

fn body_lines(card: &Card, plain: bool, color: bool) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(render_title(&card.title, plain, color));
    if let Some(subtitle) = card.subtitle.as_deref() {
        lines.push(format!("  {subtitle}"));
    }
    for section in &card.sections {
        match section {
            Section::Blank => lines.push(String::new()),
            Section::Metric { label, columns } => {
                let rendered_columns = columns
                    .iter()
                    .map(|column| style_tone(&column.value, column.tone, color))
                    .collect::<Vec<_>>()
                    .join("  ");
                lines.push(format!(
                    "  {}{}",
                    pad_visible(&style_tone(label, Tone::Dim, color), LABEL_WIDTH),
                    rendered_columns
                ));
            }
            Section::KeyValue { rows } => {
                let key_width = rows
                    .iter()
                    .map(|(key, _)| visible_length(key))
                    .max()
                    .unwrap_or(0)
                    .max(LABEL_WIDTH - 2);
                for (key, value) in rows {
                    lines.push(format!(
                        "  {}  {}",
                        pad_visible(&style_tone(key, Tone::Dim, color), key_width),
                        value
                    ));
                }
            }
            Section::Lines {
                lines: section_lines,
            } => {
                for line in section_lines {
                    lines.push(format!("  {line}"));
                }
            }
            Section::Command { label, command } => {
                lines.push(format!(
                    "  {}{}",
                    pad_visible(&style_tone(label, Tone::Dim, color), LABEL_WIDTH),
                    style_tone(command, Tone::Neutral, color)
                ));
            }
        }
    }
    if card.primary_action.is_some() || !card.hints.is_empty() {
        lines.push(String::new());
    }
    if let Some(primary) = card.primary_action.as_ref() {
        lines.push(format!(
            "  {}{}",
            pad_visible(&style_tone(&primary.label, Tone::Good, color), LABEL_WIDTH),
            style_tone(&primary.command, Tone::Good, color)
        ));
    }
    if !card.hints.is_empty() {
        for hint in &card.hints {
            lines.push(format!(
                "  {}{}",
                pad_visible(&style_tone(&hint.label, Tone::Dim, color), LABEL_WIDTH),
                style_tone(&hint.command, Tone::Neutral, color)
            ));
        }
    }
    lines
}

fn render_title(title: &TitleLine, plain: bool, color: bool) -> String {
    let glyph = match (title.glyph, plain) {
        (TitleGlyph::Success, false) => "✦",
        (TitleGlyph::Success, true) => "*",
        (TitleGlyph::Stopped, false) => "×",
        (TitleGlyph::Stopped, true) => "x",
        (TitleGlyph::Paused, false) => "⧖",
        (TitleGlyph::Paused, true) => "~",
        (TitleGlyph::Failed, false) => "⊘",
        (TitleGlyph::Failed, true) => "!",
        (TitleGlyph::Preview, false) => "▸",
        (TitleGlyph::Preview, true) => ">",
    };
    let tone = match title.glyph {
        TitleGlyph::Success | TitleGlyph::Preview => Tone::Good,
        TitleGlyph::Paused => Tone::Warn,
        TitleGlyph::Stopped | TitleGlyph::Failed => Tone::Bad,
    };
    format!(
        "{} {}",
        style_tone(glyph, tone, color),
        style_title(&title.label, color)
    )
}

fn render_narrow(lines: &[String], plain: bool) -> String {
    let mut out = String::new();
    for line in lines {
        let plain_line = if plain {
            strip_ansi(line)
        } else {
            strip_ansi(line).replace('│', "|")
        };
        out.push_str(plain_line.trim_end());
        out.push('\n');
    }
    out
}

fn border(position: &str, width: usize, plain: bool, color: bool) -> String {
    let count = width.saturating_sub(2);
    let line = if plain {
        match position {
            "top" | "bottom" => format!("+{}+", "-".repeat(count)),
            _ => String::new(),
        }
    } else {
        match position {
            "top" => format!("╭{}╮", "─".repeat(count)),
            "bottom" => format!("╰{}╯", "─".repeat(count)),
            _ => String::new(),
        }
    };
    style_border(&line, color)
}

fn style_border(text: &str, color: bool) -> String {
    if color {
        crate::ui::ansi_wrap("2", text)
    } else {
        text.to_string()
    }
}

fn style_title(text: &str, color: bool) -> String {
    if color {
        crate::ui::ansi_wrap("1", text)
    } else {
        text.to_string()
    }
}

fn style_tone(text: &str, tone: Tone, color: bool) -> String {
    if !color {
        return text.to_string();
    }
    let code = match tone {
        Tone::Neutral => "36",
        Tone::Good => "32",
        Tone::Warn => "33",
        Tone::Bad => "31",
        Tone::Dim => "2",
    };
    crate::ui::ansi_wrap(code, text)
}

fn truncate_visible_for_mode(text: &str, width: usize, plain: bool) -> String {
    if plain {
        truncate_visible_inner(&strip_ansi(text), width, "...")
    } else {
        truncate_visible(text, width)
    }
}

fn truncate_visible_inner(text: &str, width: usize, ellipsis: &str) -> String {
    use unicode_width::UnicodeWidthChar;
    if visible_length(text) <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let ellipsis_width = crate::ui::display_width(ellipsis);
    if width <= ellipsis_width {
        let mut out = String::new();
        let mut used = 0usize;
        for ch in ellipsis.chars() {
            let glyph_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + glyph_width > width {
                break;
            }
            out.push(ch);
            used += glyph_width;
        }
        return out;
    }

    let target_width = width - ellipsis_width;
    let mut out = String::new();
    let mut visible = 0usize;
    let mut active_style = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some((sequence, active)) = crate::ui::read_ansi_sequence(ch, &mut chars) {
            active_style = active;
            out.push_str(&sequence);
            continue;
        }
        let glyph_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if visible + glyph_width > target_width {
            break;
        }
        out.push(ch);
        visible += glyph_width;
    }
    out.push_str(ellipsis);
    if active_style {
        out.push_str(crate::ui::ansi_reset());
    }
    out
}
