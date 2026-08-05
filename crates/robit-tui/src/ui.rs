//! UI rendering — draws the TUI layout using ratatui.
//!
//! Rendering model:
//!
//! - Committed conversation entries are rendered (Markdown parsing, tool-card
//!   construction, JSON parsing) and **pre-wrapped into visual lines exactly
//!   once**, then cached in [`RenderCache`]. Frames only re-wrap the streaming
//!   tail and slice the visible window, so per-frame cost is O(viewport)
//!   instead of O(history).
//! - All scrolling math (auto-scroll, manual offset, scrollbar) operates on
//!   the pre-wrapped visual lines — the same unit that is actually displayed —
//!   so the bottom of the content is always reachable and nothing is clipped.
//! - `scroll_offset` semantics: distance from the bottom in visual lines.
//!   `0` means "pinned to the latest content" (auto-scroll).

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarState};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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

pub fn draw(f: &mut Frame, app: &mut App) {
    let size = f.area();

    let input_h = input_height(app, size.width);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),       // Status bar
            Constraint::Min(5),          // Conversation
            Constraint::Length(input_h), // Input area
        ])
        .split(size);

    draw_status_bar(f, app, chunks[0]);
    draw_conversation(f, app, chunks[1]);
    draw_input(f, app, chunks[2]);

    if matches!(app.input_mode, InputMode::Confirmation { .. }) {
        draw_confirmation_overlay(f, size, input_h);
    }
}

/// Height of the input area, based on the **wrapped** visual rows of the
/// current content (not the logical newline count), so long wrapped lines get
/// the space they need.
fn input_height(app: &App, width: u16) -> u16 {
    let inner_width = (width as usize).saturating_sub(2).max(1);
    let content = app.input.content();
    let mut rows = if content.is_empty() {
        1
    } else {
        content
            .split('\n')
            .map(|seg| wrapped_height(seg, inner_width))
            .sum::<usize>()
    };
    // Reserve a row for the cursor when it sits at the start of the row just
    // past the rendered text (content ends exactly on the wrap width).
    if needs_cursor_row(content, app.input.cursor(), inner_width) {
        rows += 1;
    }
    (rows as u16 + 2).clamp(3, 8)
}

/// True when the cursor occupies a visual row that has no rendered text yet:
/// the content is non-empty, the cursor is at its end, and the last logical
/// line's width is a non-zero multiple of the wrap width.
fn needs_cursor_row(content: &str, cursor: usize, width: usize) -> bool {
    if content.is_empty() || cursor != content.len() {
        return false;
    }
    let last_seg = content.rsplit('\n').next().unwrap_or("");
    let w = UnicodeWidthStr::width(last_seg);
    w > 0 && w.is_multiple_of(width.max(1))
}

/// Number of visual rows a single logical line occupies when wrapped at
/// `width` columns.
fn wrapped_height(seg: &str, width: usize) -> usize {
    let w = UnicodeWidthStr::width(seg);
    if w == 0 {
        1
    } else {
        w.div_ceil(width.max(1))
    }
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
            format!("robit v{}", env!("CARGO_PKG_VERSION")),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            match &app.status.image_model {
                Some(img) => format!("{} / {}", app.status.model, img),
                None => app.status.model.clone(),
            },
            Style::default().fg(Color::Cyan),
        ),
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
// Render cache for committed conversation entries
// ============================================================================

/// Cache of rendered visual lines for committed conversation entries.
///
/// Committed entries change rarely (a new entry is appended, or a tool card's
/// status updates), while frames arrive many times per second during
/// streaming. The cache renders and pre-wraps each entry once and reuses the
/// result across frames.
#[derive(Default)]
pub struct RenderCache {
    /// Number of conversation entries covered by the cache.
    count: usize,
    /// Card width the cache was built with.
    card_width: usize,
    /// Wrap width the cache was built with.
    wrap_width: usize,
    /// Flattened pre-wrapped visual lines for entries `[0..count)`.
    lines: Vec<Line<'static>>,
    /// Line range of each cached entry inside `lines`.
    ranges: Vec<std::ops::Range<usize>>,
    /// Indices of entries that must be re-rendered on the next sync.
    dirty: Vec<usize>,
}

impl RenderCache {
    /// Mark entry `idx` as needing a re-render on the next [`sync`](Self::sync).
    pub fn invalidate(&mut self, idx: usize) {
        if idx < self.count && !self.dirty.contains(&idx) {
            self.dirty.push(idx);
        }
    }

    fn reset(&mut self) {
        self.count = 0;
        self.lines.clear();
        self.ranges.clear();
        self.dirty.clear();
    }

    /// Synchronize the cache with the conversation and return the flattened
    /// visual lines of all committed entries.
    pub fn sync<'a>(
        &'a mut self,
        conversation: &[ConversationEntry],
        card_width: usize,
        wrap_width: usize,
    ) -> &'a [Line<'static>] {
        // Width changes invalidate every entry (cards and wrapping depend on it).
        if self.card_width != card_width || self.wrap_width != wrap_width {
            self.reset();
            self.card_width = card_width;
            self.wrap_width = wrap_width;
        }
        // Conversation shrunk (e.g. /clear) — start over.
        if conversation.len() < self.count {
            self.reset();
        }

        // Re-render dirty entries (e.g. tool card status changes).
        for idx in std::mem::take(&mut self.dirty) {
            if idx >= conversation.len() || idx >= self.ranges.len() {
                continue;
            }
            let mut fresh = Vec::new();
            render_entry_wrapped(&mut fresh, &conversation[idx], card_width, wrap_width);
            let start = self.ranges[idx].start;
            let old_len = self.ranges[idx].end - start;
            let fresh_len = fresh.len();
            self.lines.splice(start..start + old_len, fresh);
            self.ranges[idx] = start..start + fresh_len;
            let delta = fresh_len as isize - old_len as isize;
            for range in self.ranges[idx + 1..].iter_mut() {
                if delta >= 0 {
                    range.start += delta as usize;
                    range.end += delta as usize;
                } else {
                    let d = (-delta) as usize;
                    range.start -= d;
                    range.end -= d;
                }
            }
        }

        // Render newly appended entries.
        for entry in conversation.iter().skip(self.count) {
            let start = self.lines.len();
            render_entry_wrapped(&mut self.lines, entry, card_width, wrap_width);
            self.ranges.push(start..self.lines.len());
        }
        self.count = conversation.len();

        &self.lines
    }
}

// ============================================================================
// Conversation pane
// ============================================================================

fn draw_conversation(f: &mut Frame, app: &mut App, area: Rect) {
    let visible_height = area.height as usize;
    if visible_height == 0 || area.width == 0 {
        return;
    }

    // Reserve the rightmost column for the scrollbar so it never covers text.
    let wrap_width = (area.width as usize).saturating_sub(1).max(1);
    // Card content width: card + its 2 border chars must fit within wrap_width.
    let card_width = wrap_width.saturating_sub(2).clamp(1, 100);

    // Streaming tail (not yet committed) — re-rendered every frame.
    let mut tail_logical: Vec<Line<'static>> = Vec::new();
    if !app.current_assistant_text.is_empty() {
        tail_logical.push(assistant_header());
        tail_logical.extend(render_markdown(&app.current_assistant_text));
    } else if app.is_agent_busy {
        tail_logical.push(Line::from(Span::styled(
            "  ⏳ Thinking...",
            Style::default().fg(Color::DarkGray),
        )));
    }
    let mut tail: Vec<Line<'static>> = Vec::new();
    for line in &tail_logical {
        wrap_line(line, wrap_width, &mut tail);
    }

    // Welcome screen when there is nothing to show yet.
    if app.conversation.is_empty() && tail.is_empty() {
        tail.push(Line::from(""));
        tail.push(Line::from(Span::styled(
            "  Welcome to Robit — AI Automaton Agent",
            Style::default().fg(Color::DarkGray),
        )));
        tail.push(Line::from(Span::styled(
            "  Type a message to start, /exit to quit",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let (visible, total, scroll) = {
        let committed = app.render_cache.sync(&app.conversation, card_width, wrap_width);
        let total = committed.len() + tail.len();
        let max_scroll = total.saturating_sub(visible_height);

        // Clamp stale offsets (content may have shrunk, e.g. after /clear).
        if app.scroll_offset > max_scroll {
            app.scroll_offset = max_scroll;
        }
        // Offset 0 means "at the bottom" — re-enable auto-follow.
        if app.scroll_offset == 0 && !app.auto_scroll {
            app.auto_scroll = true;
        }

        // scroll_offset is the distance from the bottom; convert it into the
        // number of visual lines to skip from the top.
        let scroll = if app.auto_scroll {
            max_scroll
        } else {
            max_scroll.saturating_sub(app.scroll_offset)
        };

        let visible: Vec<Line<'static>> = committed
            .iter()
            .chain(tail.iter())
            .skip(scroll)
            .take(visible_height)
            .cloned()
            .collect();
        (visible, total, scroll)
    };

    // Lines are pre-wrapped: no `Wrap` here, so scroll math and rendering
    // agree exactly (ratatui's wrap mode counts scroll in wrapped lines).
    f.render_widget(Paragraph::new(visible), area);

    if total > visible_height {
        // ratatui's Scrollbar treats `position` as a cursor index in
        // `0..content_length`, with the thumb flush at the end only when
        // position == content_length - 1. Passing the number of scroll stops
        // (total - viewport + 1) as content_length makes `scroll` fit exactly
        // that range, and the math reduces to a proportional mapping:
        //   thumb_len   = viewport / total * track
        //   thumb_start = scroll   / total * track
        let mut state = ScrollbarState::new(total - visible_height + 1)
            .position(scroll)
            .viewport_content_length(visible_height);
        f.render_stateful_widget(
            Scrollbar::default()
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("░"))
                .thumb_style(Style::default().fg(Color::Indexed(240)))
                .track_style(Style::default().fg(Color::DarkGray)),
            area,
            &mut state,
        );
    }
}

fn assistant_header() -> Line<'static> {
    Line::from(Span::styled(
        "🤖 Robit:",
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    ))
}

// ============================================================================
// Pre-wrapping
// ============================================================================

/// Wrap a logical line into visual lines of at most `width` columns.
///
/// Always produces at least one line. A wide character that no longer fits on
/// the current row starts a new row. Character-level wrapping keeps the math
/// simple and exact for CJK text; it also matches the width-based truncation
/// already used by tool cards.
fn wrap_line(line: &Line<'_>, width: usize, out: &mut Vec<Line<'static>>) {
    let width = width.max(1);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut col = 0usize;

    for span in &line.spans {
        let style = span.style;
        let mut chunk = String::new();
        for c in span.content.chars() {
            let cw = UnicodeWidthChar::width(c).unwrap_or(0);
            if col + cw > width {
                if !chunk.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut chunk), style));
                }
                out.push(Line::from(std::mem::take(&mut spans)));
                col = 0;
            }
            chunk.push(c);
            col += cw;
        }
        if !chunk.is_empty() {
            spans.push(Span::styled(chunk, style));
        }
    }
    out.push(Line::from(spans));
}

/// Render one conversation entry and pre-wrap it into visual lines.
fn render_entry_wrapped(
    out: &mut Vec<Line<'static>>,
    entry: &ConversationEntry,
    card_width: usize,
    wrap_width: usize,
) {
    let mut logical: Vec<Line<'static>> = Vec::new();
    render_entry(&mut logical, entry, card_width);
    for line in &logical {
        wrap_line(line, wrap_width, out);
    }
}

// ============================================================================
// Render conversation entries into flat (logical) lines
// ============================================================================

fn render_entry(lines: &mut Vec<Line<'static>>, entry: &ConversationEntry, card_width: usize) {
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
            lines.push(assistant_header());
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

fn render_tool_card(
    lines: &mut Vec<Line<'static>>,
    name: &str,
    arguments: &str,
    status: &ToolStatus,
    card_width: usize,
) {
    let (icon, color) = match status {
        ToolStatus::Pending => ("⏳", Color::DarkGray),
        ToolStatus::Running => ("⏳", Color::Yellow),
        ToolStatus::Success(_) => ("✓", Color::Green),
        ToolStatus::Failed(_) => ("✗", Color::Red),
        ToolStatus::Rejected => ("⊘", Color::DarkGray),
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
                            *w += UnicodeWidthChar::width(c).unwrap_or(0);
                            if *w <= card_width.saturating_sub(1) {
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
        ToolStatus::AwaitingConfirmation => " ⚠ Awaiting confirmation (Y/N)...".to_string(),
    };

    let sw = UnicodeWidthStr::width(status_line.as_str());
    if sw > card_width {
        let truncated: String = status_line
            .chars()
            .scan(0, |w, c| {
                *w += UnicodeWidthChar::width(c).unwrap_or(0);
                if *w <= card_width.saturating_sub(1) {
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
                " {}Enter Send | Tab Multi-line | F8 Scroll | Ctrl+D Cancel | Ctrl+C Exit{}",
                mode_indicator, ""
            ),
            Style::default().fg(Color::DarkGray),
        )));

    let inner_width = (area.width as usize).saturating_sub(2).max(1);
    let content = app.input.content();

    let mut lines: Vec<Line<'static>> = Vec::new();
    if content.is_empty() {
        let placeholder = if matches!(app.input_mode, InputMode::Confirmation { .. }) {
            "Press Y to allow or N to deny..."
        } else {
            "Type a message..."
        };
        lines.push(Line::from(Span::styled(
            placeholder,
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        // Pre-wrap here (instead of Paragraph::wrap) so the rendered rows
        // match input_height() and visual_cursor() exactly.
        for seg in content.split('\n') {
            let line = Line::from(Span::styled(
                seg.to_string(),
                Style::default().fg(Color::White),
            ));
            wrap_line(&line, inner_width, &mut lines);
        }
        // Mirror input_height(): give the cursor an empty row to sit on when
        // the content ends exactly on the wrap width.
        if needs_cursor_row(content, app.input.cursor(), inner_width) {
            lines.push(Line::from(""));
        }
    }

    f.render_widget(Paragraph::new(lines).block(block), area);

    // The cursor must be set on every frame in normal mode: ratatui hides the
    // terminal cursor for any frame in which `set_cursor_position` is not
    // called — including frames where the input is empty (placeholder shown).
    if matches!(app.input_mode, InputMode::Normal) {
        let inner = area.inner(ratatui::layout::Margin {
            horizontal: 1,
            vertical: 1,
        });
        let (row, col) = if content.is_empty() {
            (0u16, 0u16)
        } else {
            visual_cursor(content, app.input.cursor(), inner_width).unwrap_or((0, 0))
        };
        if row < inner.height {
            f.set_cursor_position((inner.x + col, inner.y + row));
        }
    }
}

/// Visual (row, col) of the cursor within input content wrapped at `width`.
fn visual_cursor(content: &str, cursor: usize, width: usize) -> Option<(u16, u16)> {
    let cursor = cursor.min(content.len());
    let prefix = &content[..cursor];
    let line_start = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let width = width.max(1);

    // Full logical lines before the cursor line contribute their wrapped height.
    let mut row = 0usize;
    if line_start > 0 {
        for seg in content[..line_start - 1].split('\n') {
            row += wrapped_height(seg, width);
        }
    }
    // Position within the current (possibly wrapped) line.
    let col_width = UnicodeWidthStr::width(&prefix[line_start..]);
    row += col_width / width;
    let col = col_width % width;

    Some((row.min(u16::MAX as usize) as u16, col as u16))
}

// ============================================================================
// Confirmation overlay
// ============================================================================

fn draw_confirmation_overlay(f: &mut Frame, full_area: Rect, input_h: u16) {
    // Calculate conversation area (exclude status bar + input area)
    let conv_area = Rect::new(
        full_area.x,
        full_area.y + 1,
        full_area.width,
        full_area.height.saturating_sub(1 + input_h),
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(s: &str) -> Line<'static> {
        Line::from(Span::raw(s.to_string()))
    }

    fn wrap_to_strings(line: &Line<'static>, width: usize) -> Vec<String> {
        let mut out = Vec::new();
        wrap_line(line, width, &mut out);
        out.iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    fn visual_width(l: &Line<'static>) -> usize {
        l.spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum()
    }

    #[test]
    fn wrap_line_keeps_short_lines_intact() {
        let out = wrap_to_strings(&plain("hello"), 10);
        assert_eq!(out, vec!["hello"]);
    }

    #[test]
    fn wrap_line_wraps_ascii_at_width() {
        let out = wrap_to_strings(&plain("abcdefghij"), 4);
        assert_eq!(out, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wrap_line_handles_wide_chars() {
        // Each CJK char is 2 columns; width 5 fits two (4 cols) per row.
        let out = wrap_to_strings(&plain("你好你好你"), 5);
        assert_eq!(out, vec!["你好", "你好", "你"]);
        // No row may exceed the width.
        let mut wrapped = Vec::new();
        wrap_line(&plain("你好你好你"), 5, &mut wrapped);
        for l in &wrapped {
            assert!(visual_width(l) <= 5);
        }
    }

    #[test]
    fn wrap_line_empty_produces_one_line() {
        let out = wrap_to_strings(&plain(""), 10);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], "");
    }

    #[test]
    fn needs_cursor_row_only_at_exact_wrap_boundary_end() {
        // 6 chars with width 3: content ends exactly on the boundary, cursor
        // at the end → needs an extra row.
        assert!(needs_cursor_row("abcdef", 6, 3));
        // Cursor not at the end → following text provides the row.
        assert!(!needs_cursor_row("abcdef", 3, 3));
        // Width not an exact multiple → cursor sits on the last rendered row.
        assert!(!needs_cursor_row("abcde", 5, 3));
        // Empty content → placeholder row is used instead.
        assert!(!needs_cursor_row("", 0, 3));
        // Trailing newline: last logical line is empty, width 0.
        assert!(!needs_cursor_row("abc\n", 4, 3));
    }

    #[test]
    fn wrapped_height_counts_visual_rows() {
        assert_eq!(wrapped_height("", 10), 1);
        assert_eq!(wrapped_height("abc", 10), 1);
        assert_eq!(wrapped_height("abcdefghijkl", 10), 2);
        assert_eq!(wrapped_height("你好你好你好", 5), 3); // 12 cols / 5
    }

    #[test]
    fn visual_cursor_first_line() {
        assert_eq!(visual_cursor("hello", 0, 10), Some((0, 0)));
        assert_eq!(visual_cursor("hello", 3, 10), Some((0, 3)));
    }

    #[test]
    fn visual_cursor_wraps() {
        // Width 4: cursor after 6 chars is on row 1, col 2.
        assert_eq!(visual_cursor("abcdefgh", 6, 4), Some((1, 2)));
        // Exactly at the row end: cursor goes to the next row start.
        assert_eq!(visual_cursor("abcdefgh", 4, 4), Some((1, 0)));
    }

    #[test]
    fn visual_cursor_multiline() {
        // "ab\ncde" is 6 bytes; cursor at the very end: second row, col 3.
        assert_eq!(visual_cursor("ab\ncde", 6, 10), Some((1, 3)));
        // Cursor between 'c' and 'd' (byte 4): second row, col 1.
        assert_eq!(visual_cursor("ab\ncde", 4, 10), Some((1, 1)));
        // Cursor at start of second line (3).
        assert_eq!(visual_cursor("ab\ncde", 3, 10), Some((1, 0)));
    }

    fn text_of(lines: &[Line<'static>]) -> String {
        let mut s = String::new();
        for l in lines {
            for span in &l.spans {
                s.push_str(span.content.as_ref());
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn render_cache_appends_invalidates_and_clears() {
        use crate::app::{ConversationEntry, ToolStatus};

        let mut cache = RenderCache::default();
        let width = 60usize;

        let mut entries = vec![
            ConversationEntry::UserMessage("hello world".to_string()),
            ConversationEntry::ToolCard {
                tool_call_id: "t1".to_string(),
                name: "bash".to_string(),
                arguments: "{\"command\":\"ls\"}".to_string(),
                status: ToolStatus::Pending,
            },
        ];

        // Initial render.
        let text = text_of(cache.sync(&entries, width, width));
        assert!(text.contains("hello world"));
        assert!(text.contains("Pending"));
        let len_after_two = cache.lines.len();
        assert!(len_after_two > 0);
        // Ranges must exactly cover the flattened lines.
        assert_eq!(cache.ranges.last().unwrap().end, len_after_two);

        // Sync without changes: identical output.
        assert_eq!(cache.sync(&entries, width, width).len(), len_after_two);

        // Status change + invalidate: entry 1 re-rendered in place.
        if let ConversationEntry::ToolCard { status, .. } = &mut entries[1] {
            *status = ToolStatus::Success("all good".to_string());
        }
        cache.invalidate(1);
        let text = text_of(cache.sync(&entries, width, width));
        assert!(text.contains("Done"));
        assert!(!text.contains("Pending"));
        // Entry 0's lines are untouched: still start at line 0.
        assert_eq!(cache.ranges[0].start, 0);
        assert!(text_of(&cache.lines[cache.ranges[0].clone()]).contains("hello world"));

        // Append a new entry.
        entries.push(ConversationEntry::SystemNotice("notice here".to_string()));
        let text = text_of(cache.sync(&entries, width, width));
        assert!(text.contains("notice here"));
        assert_eq!(cache.ranges.len(), 3);
        assert_eq!(cache.ranges.last().unwrap().end, cache.lines.len());

        // Shrink (e.g. /clear) resets the cache.
        assert_eq!(cache.sync(&[], width, width).len(), 0);
        entries.clear();
        entries.push(ConversationEntry::UserMessage("fresh start".to_string()));
        let text = text_of(cache.sync(&entries, width, width));
        assert!(text.contains("fresh start"));
        assert!(!text.contains("hello world"));

        // Width change forces a full rebuild (long lines wrap differently).
        entries.push(ConversationEntry::UserMessage(
            "a very very very long single line of text".to_string(),
        ));
        let wide = text_of(cache.sync(&entries, width, width));
        cache.invalidate(0); // also prove dirty indices survive nothing here
        let narrow = text_of(cache.sync(&entries, 20, 20));
        assert!(narrow.lines().count() > wide.lines().count());
    }
}
