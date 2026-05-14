const DEFAULT_CARD_WIDTH: usize = 80;
const MIN_CARD_WIDTH: usize = 62;
const NARROW_FALLBACK_WIDTH: usize = 40;
const LABEL_WIDTH: usize = 14;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub title: TitleLine,
    pub subtitle: Option<String>,
    pub sections: Vec<Section>,
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
    strip_ansi(text).chars().count()
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
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if code.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
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
                    .map(|(key, _)| key.chars().count())
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
            Section::Command { label, command } => {
                lines.push(format!(
                    "  {}{}",
                    pad_visible(&style_tone(label, Tone::Dim, color), LABEL_WIDTH),
                    style_tone(command, Tone::Neutral, color)
                ));
            }
        }
    }
    if !card.hints.is_empty() {
        lines.push(String::new());
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
        format!("\u{1b}[2m{text}\u{1b}[0m")
    } else {
        text.to_string()
    }
}

fn style_title(text: &str, color: bool) -> String {
    if color {
        format!("\u{1b}[1m{text}\u{1b}[0m")
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
    format!("\u{1b}[{code}m{text}\u{1b}[0m")
}

fn truncate_visible_for_mode(text: &str, width: usize, plain: bool) -> String {
    if plain {
        truncate_visible_inner(&strip_ansi(text), width, "...")
    } else {
        truncate_visible(text, width)
    }
}

fn truncate_visible_inner(text: &str, width: usize, ellipsis: &str) -> String {
    if visible_length(text) <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let ellipsis_width = ellipsis.chars().count();
    if width <= ellipsis_width {
        return ellipsis.chars().take(width).collect();
    }

    let target_width = width - ellipsis_width;
    let mut out = String::new();
    let mut visible = 0usize;
    let mut active_style = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            let mut sequence = String::from(ch);
            if let Some(bracket) = chars.next() {
                sequence.push(bracket);
            }
            for code in chars.by_ref() {
                sequence.push(code);
                if code.is_ascii_alphabetic() {
                    active_style = sequence != "\u{1b}[0m";
                    break;
                }
            }
            out.push_str(&sequence);
            continue;
        }
        if visible >= target_width {
            break;
        }
        out.push(ch);
        visible += 1;
    }
    out.push_str(ellipsis);
    if active_style {
        out.push_str("\u{1b}[0m");
    }
    out
}
