use pulldown_cmark::{
    CodeBlockKind, Event as MarkdownEvent, HeadingLevel, Options as MarkdownOptions,
    Parser as MarkdownParser, Tag, TagEnd,
};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkdownBlock {
    Paragraph,
    Heading(HeadingLevel),
    Item,
}

pub(crate) fn markdown_to_tui_lines(markdown: &str) -> Vec<Line<'static>> {
    let options = MarkdownOptions::ENABLE_TABLES
        | MarkdownOptions::ENABLE_STRIKETHROUGH
        | MarkdownOptions::ENABLE_TASKLISTS;
    let parser = MarkdownParser::new_ext(markdown, options);
    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut block: Option<MarkdownBlock> = None;
    let mut inline_style = Style::default();
    let mut code_block = false;

    for event in parser {
        match event {
            MarkdownEvent::Start(Tag::Heading { level, .. }) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                block = Some(MarkdownBlock::Heading(level));
            }
            MarkdownEvent::End(TagEnd::Heading(_)) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                lines.push(Line::raw(""));
            }
            MarkdownEvent::Start(Tag::Paragraph) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                block = Some(MarkdownBlock::Paragraph);
            }
            MarkdownEvent::End(TagEnd::Paragraph) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                lines.push(Line::raw(""));
            }
            MarkdownEvent::Start(Tag::Item) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                current.push(Span::styled("  - ", Style::default().fg(Color::Cyan)));
                block = Some(MarkdownBlock::Item);
            }
            MarkdownEvent::End(TagEnd::Item) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
            }
            MarkdownEvent::Start(Tag::CodeBlock(kind)) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                let language = match kind {
                    CodeBlockKind::Fenced(language) if !language.is_empty() => {
                        format!(" {}", language)
                    }
                    _ => String::new(),
                };
                lines.push(Line::styled(
                    format!("```{language}"),
                    Style::default().fg(Color::DarkGray),
                ));
                code_block = true;
            }
            MarkdownEvent::End(TagEnd::CodeBlock) => {
                code_block = false;
                lines.push(Line::styled("```", Style::default().fg(Color::DarkGray)));
                lines.push(Line::raw(""));
            }
            MarkdownEvent::Start(Tag::Strong) => {
                inline_style = inline_style.add_modifier(Modifier::BOLD);
            }
            MarkdownEvent::End(TagEnd::Strong) => {
                inline_style = inline_style.remove_modifier(Modifier::BOLD);
            }
            MarkdownEvent::Start(Tag::Emphasis) => {
                inline_style = inline_style.add_modifier(Modifier::ITALIC);
            }
            MarkdownEvent::End(TagEnd::Emphasis) => {
                inline_style = inline_style.remove_modifier(Modifier::ITALIC);
            }
            MarkdownEvent::Start(Tag::Link { dest_url, .. }) => {
                inline_style = inline_style
                    .fg(Color::Blue)
                    .add_modifier(Modifier::UNDERLINED);
                if !dest_url.is_empty() {
                    current.push(Span::styled(
                        "",
                        Style::default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::UNDERLINED),
                    ));
                }
            }
            MarkdownEvent::End(TagEnd::Link) => {
                inline_style = Style::default();
            }
            MarkdownEvent::Text(text) => {
                if code_block {
                    for line in text.lines() {
                        lines.push(Line::styled(
                            format!("  {line}"),
                            Style::default().fg(Color::LightGreen),
                        ));
                    }
                } else {
                    current.push(Span::styled(text.into_string(), inline_style));
                }
            }
            MarkdownEvent::Code(code) => current.push(Span::styled(
                code.into_string(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            MarkdownEvent::SoftBreak => current.push(Span::raw(" ")),
            MarkdownEvent::HardBreak => {
                flush_markdown_line(&mut lines, &mut current, block);
                block = Some(MarkdownBlock::Paragraph);
            }
            MarkdownEvent::Rule => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                lines.push(Line::styled(
                    "────────────────────────────────────────",
                    Style::default().fg(Color::DarkGray),
                ));
            }
            MarkdownEvent::Html(html) | MarkdownEvent::InlineHtml(html) => current.push(
                Span::styled(html.into_string(), Style::default().fg(Color::DarkGray)),
            ),
            MarkdownEvent::InlineMath(math) => current.push(Span::styled(
                math.into_string(),
                Style::default().fg(Color::Magenta),
            )),
            MarkdownEvent::DisplayMath(math) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                lines.push(Line::styled(
                    math.into_string(),
                    Style::default().fg(Color::Magenta),
                ));
            }
            MarkdownEvent::Start(_)
            | MarkdownEvent::End(_)
            | MarkdownEvent::FootnoteReference(_) => {}
            MarkdownEvent::TaskListMarker(checked) => current.push(Span::styled(
                if checked { "[x] " } else { "[ ] " },
                Style::default().fg(Color::Cyan),
            )),
        }
    }
    flush_markdown_line(&mut lines, &mut current, block.take());
    if lines.is_empty() {
        lines.push(Line::styled(
            "Narrative docs are empty.",
            Style::default().fg(Color::Yellow),
        ));
    }
    lines
}

fn flush_markdown_line(
    lines: &mut Vec<Line<'static>>,
    current: &mut Vec<Span<'static>>,
    block: Option<MarkdownBlock>,
) {
    if current.is_empty() {
        return;
    }
    let style = match block.unwrap_or(MarkdownBlock::Paragraph) {
        MarkdownBlock::Heading(HeadingLevel::H1) => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        MarkdownBlock::Heading(HeadingLevel::H2) => Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
        MarkdownBlock::Heading(_) => Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
        MarkdownBlock::Item => Style::default().fg(Color::White),
        MarkdownBlock::Paragraph => Style::default(),
    };
    let mut spans = Vec::new();
    if matches!(block, Some(MarkdownBlock::Heading(level)) if level != HeadingLevel::H1) {
        spans.push(Span::styled("▸ ", Style::default().fg(Color::Cyan)));
    }
    spans.extend(current.drain(..).map(|span| span.patch_style(style)));
    lines.push(Line::from(spans));
}
