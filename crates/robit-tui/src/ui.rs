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
            Constraint::Length(1),  // Status bar
            Constraint::Min(5),    // Conversation
            Constraint::Length(input_height(app)), // Input area
        ])
        .split(size);

    draw_status_bar(f, app, chunks[0]);
    draw_conversation(f, app, chunks[1]);
    draw_input(f, app, chunks[2]);

    // Confirmation overlay
    if let InputMode::Confirmation { .. } = &app.input_mode {
        draw_confirmation_overlay(f, app, size);
    }
}

fn input_height(app: &App) -> u16 {
    let lines = app.input.line_count() as u16;
    // content lines + top/bottom border = lines + 2, min 3, max 8
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

    let status_text = Line::from(vec![
        Span::styled(
            format!(" {} ", indicator),
            Style::default().fg(indicator_color),
        ),
        Span::styled(
            "robit v0.1.0",
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
    ]);

    let bar = Paragraph::new(status_text).style(Style::default().bg(STATUS_BG));
    f.render_widget(bar, area);
}

// ============================================================================
// Conversation pane
// ============================================================================

fn draw_conversation(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    for entry in &app.conversation {
        render_entry(&mut lines, entry);
    }

    // Append streaming assistant text
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

    // Busy indicator
    if app.is_agent_busy && app.current_assistant_text.is_empty() {
        lines.push(Line::from(Span::styled(
            "  ⏳ 思考中...",
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Empty state
    if lines.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  欢迎使用 Robit — AI 编程代理",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            "  输入消息开始对话，/exit 退出",
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Auto-scroll: always show the bottom
    let visible_height = area.height as usize;
    let total_lines = lines.len();

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });

    if app.auto_scroll && total_lines > visible_height {
        let scroll = (total_lines - visible_height) as u16;
        let paragraph = paragraph.scroll((scroll, 0));
        f.render_widget(paragraph, area);
    } else if !app.auto_scroll && app.scroll_offset > 0 {
        let max_scroll = total_lines.saturating_sub(visible_height);
        let scroll = app.scroll_offset.min(max_scroll) as u16;
        let paragraph = paragraph.scroll((scroll, 0));
        f.render_widget(paragraph, area);
    } else {
        f.render_widget(paragraph, area);
    }
}

fn render_entry(lines: &mut Vec<Line>, entry: &ConversationEntry) {
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
            render_tool_card(lines, name, arguments, status);
        }

        ConversationEntry::Error(text) => {
            lines.push(Line::from(Span::styled(
                format!("  ✗ {}", text),
                Style::default().fg(Color::Red),
            )));
            lines.push(Line::from(""));
        }

        ConversationEntry::SystemNotice(text) => {
            lines.push(Line::from(Span::styled(
                format!("  ℹ {}", text),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )));
            lines.push(Line::from(""));
        }
    }
}

fn render_tool_card(lines: &mut Vec<Line>, name: &str, arguments: &str, status: &ToolStatus) {
    let (icon, color) = match status {
        ToolStatus::Pending => ("⏳", Color::DarkGray),
        ToolStatus::Running => ("⏳", Color::Yellow),
        ToolStatus::Success(_) => ("✓", Color::Green),
        ToolStatus::Failed(_) => ("✗", Color::Red),
        ToolStatus::Rejected => ("⊘", Color::DarkGray),
        ToolStatus::AwaitingConfirmation => ("⚠", Color::Yellow),
    };

    // Header
    let header = format!("┌─ {} {} {}", icon, name, "─".repeat(30));
    lines.push(Line::from(Span::styled(
        truncate(&header, 60),
        Style::default().fg(color),
    )));

    // Arguments
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(arguments) {
        if let Some(obj) = parsed.as_object() {
            for (k, v) in obj {
                let val_str = match v {
                    serde_json::Value::String(s) => {
                        if s.len() > 80 {
                            format!("{}...", &s[..77])
                        } else {
                            s.clone()
                        }
                    }
                    other => {
                        let s = other.to_string();
                        if s.len() > 80 {
                            format!("{}...", &s[..77])
                        } else {
                            s
                        }
                    }
                };
                lines.push(Line::from(Span::styled(
                    format!("│ {}: {}", k, val_str),
                    Style::default().fg(Color::Gray),
                )));
            }
        }
    }

    // Status line
    let status_text = match status {
        ToolStatus::Pending => "│ ⏳ 等待中...".to_string(),
        ToolStatus::Running => "│ ⏳ 执行中...".to_string(),
        ToolStatus::Success(output) => {
            let preview: String = output.lines().take(3).collect::<Vec<_>>().join("\n│ ");
            if output.lines().count() > 3 {
                format!("│ ✓ 完成\n│ {}...", preview)
            } else {
                format!("│ ✓ 完成\n│ {}", preview)
            }
        }
        ToolStatus::Failed(err) => {
            let preview: String = err.lines().take(3).collect::<Vec<_>>().join("\n│ ");
            format!("│ ✗ 失败\n│ {}", preview)
        }
        ToolStatus::Rejected => "│ ⊘ 用户拒绝".to_string(),
        ToolStatus::AwaitingConfirmation => {
            "│ [Y] 允许 / [N] 拒绝".to_string()
        }
    };

    for line in status_text.lines() {
        lines.push(Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(color),
        )));
    }

    // Footer
    lines.push(Line::from(Span::styled(
        "└────────────────────────────────────────",
        Style::default().fg(color),
    )));
    lines.push(Line::from(""));
}

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
        _ if app.input.multi_line => " [多行] ",
        _ => "",
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title_bottom(Line::from(Span::styled(
            format!(
                " {}Enter 发送 | Tab 多行 | Ctrl+C 取消 | Ctrl+D 退出{}",
                mode_indicator, ""
            ),
            Style::default().fg(Color::DarkGray),
        )));

    let input_text = if app.input.content().is_empty() {
        if matches!(app.input_mode, InputMode::Confirmation { .. }) {
            "按 Y 允许或 N 拒绝..."
        } else {
            "输入消息..."
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

    // Show cursor (only in normal mode with actual content or empty)
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

fn draw_confirmation_overlay(f: &mut Frame, _app: &App, area: Rect) {
    let popup_area = centered_rect(60, 5, area);

    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(" 确认 ");

    let text = Paragraph::new(Line::from(vec![
        Span::raw("  工具调用需要确认  "),
        Span::styled(
            "[Y] 允许",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "[N] 拒绝",
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ),
    ]))
    .block(block);

    f.render_widget(text, popup_area);
}

fn centered_rect(percent_x: u16, height: u16, r: Rect) -> Rect {
    let popup_width = r.width * percent_x / 100;
    let x = r.x + (r.width.saturating_sub(popup_width)) / 2;
    let y = r.y + (r.height.saturating_sub(height)) / 2;
    Rect::new(x, y, popup_width, height)
}

// ============================================================================
// Helpers
// ============================================================================

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}
