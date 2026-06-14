//! UI rendering — draws the TUI layout using ratatui.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, ConversationEntry, InputMode, ToolStatus};
use crate::markdown::render_markdown;

// ============================================================================
// Style constants
// ============================================================================

const STATUS_BG: Color = Color::Indexed(235);
const STATUS_FG: Color = Color::Indexed(248);
const INPUT_BORDER: Color = Color::Green;
const INPUT_BORDER_BUSY: Color = Color::Yellow;

// ============================================================================
// Main draw entry point
// ============================================================================

pub fn draw(f: &mut Frame, app: &App) {
    let size = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Status bar
            Constraint::Min(5),    // Conversation
            Constraint::Length(input_height(app)), // Input area
        ])
        .split(size);

    draw_status_bar(f, app, chunks[0]);
    draw_conversation(f, app, chunks[1]);
    draw_input(f, app, chunks[2]);

    if let InputMode::Confirmation { .. } = &app.input_mode {
        draw_confirmation_overlay(f, app, size);
    }
}

fn input_height(app: &App) -> u16 {
    let lines = app.input.line_count() as u16;
    (lines + 2).clamp(3, 8)
}

// ============================================================================
// Status bar
// ============================================================================

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let indicator = if app.is_agent_busy { "●" } else { "○" };
    let indicator_color = if app.is_agent_busy {
        Color::Yellow
    } else {
        Color::Green
    };

    let mut spans: Vec<Span> = vec![
        Span::styled(
            format!(" {} ", indicator),
            Style::default().fg(indicator_color),
        ),
        Span::styled(
            "robit v0.1.1",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        Span::styled(&app.status.model, Style::default().fg(Color::Cyan)),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!(
                "tools: {}/{}",
                app.status.tools_enabled, app.status.tools_total
            ),
            Style::default().fg(STATUS_FG),
        ),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("skills: {}", app.status.skills_total),
            Style::default().fg(STATUS_FG),
        ),
    ];

    // Scroll mode indicator at the right edge
    if app.scroll_mode {
        spans.push(Span::styled(
            " ◤SCROLL◢ F8 exit",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let status_text = Line::from(spans);

    let bar = Paragraph::new(status_text).style(Style::default().bg(STATUS_BG));
    f.render_widget(bar, area);
}

// ============================================================================
// Conversation pane
// ============================================================================

fn draw_conversation(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    // Card width: use pane width minus scrollbar (1) and borders (2), clamped to [40, 100]
    let card_width = (area.width.saturating_sub(3) as usize).clamp(40, 100);

    for entry in &app.conversation {
        render_entry(&mut lines, entry, card_width);
    }

    if !app.current_assistant_text.is_empty() {
        lines.push(Line::from(Span::styled(
            "🤖 Robit:",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )));
        let md_lines = render_markdown(&app.current_assistant_text);
        for ml in md_lines {
            lines.push(ml);
        }
    }

    if app.is_agent_busy && app.current_assistant_text.is_empty() {
        lines.push(Line::from(Span::styled(
            "  ⏳ Thinking...",
            Style::default().fg(Color::DarkGray),
        )));
    }

    if lines.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Welcome to Robit — AI Automaton Agent",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            "  Type a message to start, /exit to quit",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let visible_height = area.height as usize;
    let total_lines = lines.len();

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });

    if app.auto_scroll && total_lines > visible_height {
        let scroll = (total_lines - visible_height) as u16;
        f.render_widget(paragraph.scroll((scroll, 0)), area);
        draw_scrollbar(f, area, scroll as usize, total_lines, visible_height);
    } else if !app.auto_scroll && app.scroll_offset > 0 {
        let max_scroll = total_lines.saturating_sub(visible_height);
        let scroll = app.scroll_offset.min(max_scroll) as u16;
        f.render_widget(paragraph.scroll((scroll, 0)), area);
        draw_scrollbar(f, area, scroll as usize, total_lines, visible_height);
    } else {
        f.render_widget(paragraph, area);
    }
}

// ============================================================================
// Scrollbar
// ============================================================================

fn draw_scrollbar(f: &mut Frame, area: Rect, scroll: usize, total_lines: usize, visible_height: usize) {
    if total_lines <= visible_height || visible_height == 0 {
        return;
    }

    let max_scroll = total_lines.saturating_sub(visible_height);
    if max_scroll == 0 {
        return;
    }

    let thumb_size = ((visible_height as u64 * visible_height as u64 / total_lines as u64) as usize).max(1);
    let track = visible_height.saturating_sub(thumb_size).max(1);
    let thumb_pos = (scroll * track) / max_scroll;
    let x = area.x + area.width - 1;

    for row in 0..visible_height {
        let row_y = area.y + row as u16;
        if row_y >= area.y + area.height {
            break;
        }

        let in_thumb = row >= thumb_pos && row < thumb_pos + thumb_size;
        let ch = if in_thumb { '█' } else { '░' };
        let color = if in_thumb { Color::Indexed(240) } else { Color::DarkGray };
        let cell = Span::styled(ch.to_string(), Style::default().fg(color));
        f.render_widget(Paragraph::new(cell), Rect::new(x, row_y, 1, 1));
    }
}

// ============================================================================
// Render conversation entries into flat lines
// ============================================================================

fn render_entry(lines: &mut Vec<Line>, entry: &ConversationEntry, card_width: usize) {
    match entry {
        ConversationEntry::UserMessage(text) => {
            lines.push(Line::from(Span::styled(
                "👤 You:",
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            )));
            for line in text.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  {}", line),
                    Style::default().fg(Color::White),
                )));
            }
            lines.push(Line::from(""));
        }
        ConversationEntry::AssistantText(text) => {
            lines.push(Line::from(Span::styled(
                "🤖 Robit:",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));
            let md_lines = render_markdown(text);
            for ml in md_lines {
                lines.push(ml);
            }
            lines.push(Line::from(""));
        }
        ConversationEntry::ToolCard {
            name,
            arguments,
            status,
            ..
        } => {
            render_tool_card(lines, name, arguments, status, card_width);
        }
        ConversationEntry::Error(text) => {
            lines.push(Line::from(Span::styled(
                format!("   {}", text),
                Style::default().fg(Color::Red),
            )));
            lines.push(Line::from(""));
        }
        ConversationEntry::SystemNotice(text) => {
            lines.push(Line::from(Span::styled(
                format!("   {}", text),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )));
            lines.push(Line::from(""));
        }
    }
}

// ============================================================================
// Tool card
// ============================================================================

fn render_tool_card(lines: &mut Vec<Line>, name: &str, arguments: &str, status: &ToolStatus, card_width: usize) {
    use unicode_width::UnicodeWidthStr;

    let (icon, color) = match status {
        ToolStatus::Pending => ("⏳", Color::DarkGray),
        ToolStatus::Running => ("⏳", Color::Yellow),
        ToolStatus::Success(_) => ("✓", Color::Green),
        ToolStatus::Failed(_) => ("✗", Color::Red),
        ToolStatus::Rejected => ("", Color::DarkGray),
        ToolStatus::AwaitingConfirmation => ("⚠", Color::Yellow),
    };

    // Top border
    lines.push(Line::from(Span::styled(
        format!("┌{:─<1$}┐", "", card_width),
        Style::default().fg(color),
    )));

    // Title row
    let title = format!(" {} {} ", icon, name);
    let pad = card_width.saturating_sub(UnicodeWidthStr::width(title.as_str()));
    lines.push(Line::from(vec![
        Span::styled("│", Style::default().fg(color)),
        Span::styled(title, Style::default().fg(color)),
        Span::styled(" ".repeat(pad), Style::default().fg(color)),
        Span::styled("│", Style::default().fg(color)),
    ]));

    // Separator
    lines.push(Line::from(Span::styled(
        format!("├{:─<1$}", "", card_width),
        Style::default().fg(color),
    )));

    // Arguments
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(arguments) {
        if let Some(obj) = parsed.as_object() {
            for (k, v) in obj {
                let val_str = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let arg = format!("{}: {}", k, val_str);
                let arg_w = UnicodeWidthStr::width(arg.as_str());
                if arg_w > card_width {
                    let truncated: String = arg
                        .chars()
                        .scan(0, |w, c| {
                            *w += UnicodeWidthChar::width(c).unwrap_or(0) as usize;
                            if *w <= card_width - 1 {
                                Some(c)
                            } else {
                                None
                            }
                        })
                        .collect();
                    lines.push(Line::from(vec![
                        Span::styled("│", Style::default().fg(color)),
                        Span::styled(
                            format!(" {}…", truncated),
                            Style::default().fg(Color::Gray),
                        ),
                        Span::styled(
                            " ".repeat(card_width.saturating_sub(
                                UnicodeWidthStr::width(truncated.as_str()) + 2,
                            )),
                            Style::default().fg(color),
                        ),
                        Span::styled("│", Style::default().fg(color)),
                    ]));
                } else {
                    let pad = card_width.saturating_sub(arg_w);
                    lines.push(Line::from(vec![
                        Span::styled("│", Style::default().fg(color)),
                        Span::styled(format!(" {}", arg), Style::default().fg(Color::Gray)),
                        Span::styled(" ".repeat(pad), Style::default().fg(color)),
                        Span::styled("│", Style::default().fg(color)),
                    ]));
                }
            }
        }
    }

    // Status
    let status_line = match status {
        ToolStatus::Pending => " Pending...".to_string(),
        ToolStatus::Running => " ⏳ Running...".to_string(),
        ToolStatus::Success(output) => {
            let preview: String = output.lines().take(3).collect::<Vec<_>>().join(" | ");
            format!(" ✓ Done  {}", preview)
        }
        ToolStatus::Failed(err) => {
            let preview: String = err.lines().take(1).collect::<Vec<_>>().join("");
            format!(" ✗ Failed  {}", preview)
        }
        ToolStatus::Rejected => " ⊘ Rejected by user".to_string(),
        ToolStatus::AwaitingConfirmation => " ⏳ Awaiting confirmation...".to_string(),
    };

    let sw = UnicodeWidthStr::width(status_line.as_str());
    if sw > card_width {
        let truncated: String = status_line
            .chars()
            .scan(0, |w, c| {
                *w += UnicodeWidthChar::width(c).unwrap_or(0) as usize;
                if *w <= card_width - 1 {
                    Some(c)
                } else {
                    None
                }
            })
            .collect();
        let pad = card_width.saturating_sub(UnicodeWidthStr::width(truncated.as_str()) + 1);
        lines.push(Line::from(vec![
            Span::styled("│", Style::default().fg(color)),
            Span::styled(format!(" {}…", truncated), Style::default().fg(color)),
            Span::styled(" ".repeat(pad), Style::default().fg(color)),
            Span::styled("│", Style::default().fg(color)),
        ]));
    } else {
        let pad = card_width.saturating_sub(sw);
        lines.push(Line::from(vec![
            Span::styled("│", Style::default().fg(color)),
            Span::styled(status_line, Style::default().fg(color)),
            Span::styled(" ".repeat(pad), Style::default().fg(color)),
            Span::styled("│", Style::default().fg(color)),
        ]));
    }

    // Bottom border
    lines.push(Line::from(Span::styled(
        format!("└{:─<1$}┘", "", card_width),
        Style::default().fg(color),
    )));
    lines.push(Line::from(""));
}

use unicode_width::UnicodeWidthChar;

// ============================================================================
// Input area
// ============================================================================

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let border_color = match &app.input_mode {
        InputMode::Confirmation { .. } => Color::Yellow,
        _ if app.is_agent_busy => INPUT_BORDER_BUSY,
        _ => INPUT_BORDER,
    };

    let mode_indicator = match &app.input_mode {
        InputMode::Confirmation { .. } => " [Y/N] ",
        _ if app.input.multi_line => " [Multi-line] ",
        _ => "",
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title_bottom(Line::from(Span::styled(
            format!(
                " {}Enter Send | Tab Multi-line | F8 Scroll | Ctrl+C Cancel | Ctrl+D Exit{}",
                mode_indicator, ""
            ),
            Style::default().fg(Color::DarkGray),
        )));

    let input_text = if app.input.content().is_empty() {
        if matches!(app.input_mode, InputMode::Confirmation { .. }) {
            "Press Y to allow or N to deny..."
        } else {
            "Type a message..."
        }
    } else {
        app.input.content()
    };

    let style = if app.input.content().is_empty() {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::White)
    };

    let paragraph = Paragraph::new(input_text)
        .style(style)
        .block(block)
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);

    if matches!(app.input_mode, InputMode::Normal) {
        let inner = area.inner(ratatui::layout::Margin {
            horizontal: 1,
            vertical: 1,
        });
        let cursor_col = app.input.cursor_col();
        let cursor_row = app.input.cursor_row();
        if cursor_row < inner.height {
            f.set_cursor_position((inner.x + cursor_col, inner.y + cursor_row));
        }
    }
}

// ============================================================================
// Confirmation overlay
// ============================================================================

fn draw_confirmation_overlay(f: &mut Frame, app: &App, full_area: Rect) {
    // Calculate conversation area (exclude status bar + input area)
    let ih = input_height(app);
    let conv_area = Rect::new(
        full_area.x,
        full_area.y + 1,
        full_area.width,
        full_area.height.saturating_sub(1 + ih),
    );

    // Compact popup positioned in the upper portion of conversation area
    // — won't obscure the user message at the bottom or tool cards
    let popup_area = centered_rect_in(50, 3, conv_area);

    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(" Confirm ");

    let text = Paragraph::new(Line::from(vec![
        Span::styled(
            "[Y] Allow",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled(
            "[N] Deny",
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ),
    ]))
    .block(block);

    f.render_widget(text, popup_area);
}

/// Center a popup inside a given rect (not the full screen).
fn centered_rect_in(percent_x: u16, height: u16, r: Rect) -> Rect {
    let popup_width = r.width * percent_x / 100;
    let x = r.x + (r.width.saturating_sub(popup_width)) / 2;
    let y = r.y + (r.height.saturating_sub(height)) / 3;
    Rect::new(x, y, popup_width, height)
}
