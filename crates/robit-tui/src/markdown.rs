//! Markdown rendering — converts Markdown text to ratatui Lines using pulldown-cmark.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Render Markdown text into ratatui Lines.
pub fn render_markdown(text: &str) -> Vec<Line<'static>> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);

    let parser = Parser::new_ext(text, opts);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_line: Vec<Span<'static>> = Vec::new();
    let mut style_stack: Vec<Style> = vec![Style::default()];
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_lines: Vec<String> = Vec::new();

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { level, .. } => {
                    let hashes = "#".repeat(level as usize);
                    current_line.push(Span::styled(
                        format!("{} ", hashes),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ));
                    style_stack.push(
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    );
                }
                Tag::CodeBlock(kind) => {
                    in_code_block = true;
                    code_lines.clear();
                    code_lang = match kind {
                        pulldown_cmark::CodeBlockKind::Fenced(lang) => lang.to_string(),
                        pulldown_cmark::CodeBlockKind::Indented => String::new(),
                    };
                }
                Tag::Strong => {
                    let current = *style_stack.last().unwrap();
                    style_stack.push(current.add_modifier(Modifier::BOLD));
                }
                Tag::Emphasis => {
                    let current = *style_stack.last().unwrap();
                    style_stack.push(current.add_modifier(Modifier::ITALIC));
                }
                Tag::List(_) => {}
                Tag::Item => {
                    current_line.push(Span::styled(
                        "  • ",
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                _ => {}
            },

            Event::End(tag_end) => match tag_end {
                TagEnd::Paragraph => {
                    if !current_line.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut current_line)));
                    }
                    lines.push(Line::from(""));
                }
                TagEnd::Heading(_) => {
                    if !current_line.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut current_line)));
                    }
                    lines.push(Line::from(""));
                    style_stack.pop();
                }
                TagEnd::CodeBlock => {
                    if !code_lines.is_empty() {
                        let header = if code_lang.is_empty() {
                            "┌─ code ──────────────────────────────".to_string()
                        } else {
                            format!(
                                "┌─ {} {}─{}",
                                code_lang,
                                "─".repeat(2),
                                "─".repeat(30_usize.saturating_sub(code_lang.len()))
                            )
                        };
                        lines.push(Line::from(Span::styled(
                            header,
                            Style::default().fg(Color::DarkGray),
                        )));
                        for cl in &code_lines {
                            lines.push(Line::from(Span::styled(
                                format!("│ {}", cl),
                                Style::default().fg(Color::Gray),
                            )));
                        }
                        lines.push(Line::from(Span::styled(
                            "└────────────────────────────────────────",
                            Style::default().fg(Color::DarkGray),
                        )));
                        lines.push(Line::from(""));
                    }
                    in_code_block = false;
                    code_lang.clear();
                }
                TagEnd::Strong | TagEnd::Emphasis => {
                    style_stack.pop();
                }
                TagEnd::List(_) => {
                    lines.push(Line::from(""));
                }
                TagEnd::Item if !current_line.is_empty() => {
                    lines.push(Line::from(std::mem::take(&mut current_line)));
                }
                _ => {}
            },

            Event::Text(t) => {
                if in_code_block {
                    for line in t.split('\n') {
                        code_lines.push(line.to_string());
                    }
                } else {
                    let style = *style_stack.last().unwrap();
                    current_line.push(Span::styled(t.to_string(), style));
                }
            }

            Event::Code(c) => {
                let style = Style::default()
                    .fg(Color::White)
                    .bg(Color::Indexed(236));
                current_line.push(Span::styled(format!(" {} ", c), style));
            }

            Event::SoftBreak | Event::HardBreak if !current_line.is_empty() => {
                lines.push(Line::from(std::mem::take(&mut current_line)));
            }

            _ => {}
        }
    }

    // Flush remaining content
    if !current_line.is_empty() {
        lines.push(Line::from(current_line));
    }

    // Remove trailing empty lines
    while lines.last().is_some_and(|l| l.spans.is_empty()) {
        lines.pop();
    }

    if lines.is_empty() {
        lines.push(Line::from(""));
    }

    lines
}
