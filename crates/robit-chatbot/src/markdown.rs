//! Markdown projection for platform-specific readability.
//!
//! LLM outputs are Markdown. Each chat platform renders a subset of
//! CommonMark (WeChat iLink renders none of it). This module is **not** a
//! Markdown renderer — it is a *platform readability projection*: it converts
//! LLM Markdown into the subset described by [`MarkdownFeatures`], degrading
//! unsupported syntax to readable plain text instead of leaking raw markers.
//! All-false features therefore mean "plain-text projection", and the
//! projection runs on every platform (see `outbound::prepare_outbound`).
//!
//! Projection rules:
//!
//! - **Tables** → linearized records (`key: value` lines / `head — k: v · k: v`
//!   records). IM clients use proportional fonts, so any alignment-based grid
//!   is unreadable in principle; linearization needs no alignment at all.
//!   `features.tables = true` passes the original table source through
//!   verbatim (byte-level, via source offsets).
//! - **Downgrades respect features**: unsupported headings become plain text
//!   (or bold when `bold = true`), unsupported code fences are stripped while
//!   code content is kept, unsupported blockquotes lose the `> ` marker,
//!   unsupported links become `text (url)`, unsupported strikethrough loses
//!   its markers. List markers (`- `, `N. `) are always kept — they are a
//!   natural plain-text convention, not Markdown residue.
//! - **Task lists** (`- [ ]`) → plain unordered lists (checked state is not
//!   modeled yet).
//! - **HTML tags** → stripped entirely.
//! - **Images** → `[Image: alt]` fallback text (`![alt](url)` when
//!   `features.images = true`).
//! - **Horizontal rules** → stripped.
//!
//! # Plain-text fast path
//!
//! Text that contains no Markdown-triggering character or shape is returned
//! **byte-for-byte unchanged** (no newline normalization). This makes the
//! projection idempotent by construction for plain text and keeps system
//! notices (confirmations, hints, errors) stable through the outbound guard.
//!
//! # Idempotency
//!
//! Projecting already-projected text is a no-op: linearized records contain
//! no table syntax, degraded text contains no markers, and the fast path
//! passes plain text through untouched. Golden tests pin this byte-level.
//! Known edges (documented, rare): a record head that itself starts with a
//! block marker (`# `, `> `) is reinterpreted on a second pass.
//!
//! Parsing uses `pulldown-cmark` (already in the workspace) for safe
//! event-driven handling, so unsupported syntax inside code blocks is
//! preserved verbatim.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::adapter::MarkdownFeatures;

/// Prepare Markdown for a platform — pass through supported syntax, degrade
/// unsupported syntax to readable plain text.
///
/// `features` describes what the target platform supports. All-false features
/// produce a plain-text projection. The function is idempotent: feeding its
/// own output back in yields the same bytes (see module docs for edges).
pub fn prepare_markdown_for_platform(text: &str, features: &MarkdownFeatures) -> String {
    // Fast path: no Markdown-triggering syntax → byte-exact pass-through.
    if is_plain_text(text) {
        return text.to_string();
    }

    let mut opts = Options::empty();
    // Strikethrough parsing is always enabled so `~~x~~` can be *stripped*
    // when the platform lacks the feature; emission is gated on the flag
    // (same shape as bold/italic). With the option off, `~~` would leak
    // through as literal text.
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(text, opts);
    let mut ctx = RenderCtx::new(text.len());

    for (event, range) in parser.into_offset_iter() {
        // Verbatim table pass-through (`features.tables = true`): slice the
        // original source byte-for-byte and suppress all inner events. This
        // preserves escaped pipes (`\|`) and inline syntax exactly.
        if let Some(start) = ctx.verbatim_table {
            if matches!(event, Event::End(TagEnd::Table)) {
                ctx.verbatim_table = None;
                ctx.output.push_str(&text[start..range.end]);
                ctx.output.push_str("\n\n");
            }
            continue;
        }

        // Close a table cell: its captured inline text joins the row.
        if ctx.in_cell && matches!(event, Event::End(TagEnd::TableCell)) {
            ctx.in_cell = false;
            ctx.current_row.push(ctx.cell_buf.trim().to_string());
            continue;
        }

        // Close a downgraded link (`features.links = false`): the captured
        // link text is emitted as `text (url)` (URL preserved, no syntax).
        if ctx.link_text.is_some() && matches!(event, Event::End(TagEnd::Link)) {
            let link_text = ctx.link_text.take().unwrap();
            let url = ctx.link_url.take().unwrap_or_default();
            let rendered = if link_text.is_empty() || link_text == url {
                url
            } else {
                format!("{} ({})", link_text, url)
            };
            ctx.write(&rendered);
            continue;
        }

        match event {
            // ----- Inline text -----
            Event::Text(t) => ctx.write(&t),
            Event::Code(c) => {
                if features.inline_code {
                    ctx.write(&format!("`{}`", c));
                } else {
                    ctx.write(&c);
                }
            }
            // Hard breaks degrade to a plain newline: trailing double spaces
            // are Markdown syntax, invisible on IM clients.
            Event::SoftBreak | Event::HardBreak => {
                if ctx.link_text.is_some() || ctx.in_cell {
                    // No real newlines inside cells / link text.
                    ctx.write(" ");
                } else {
                    ctx.output.push('\n');
                    if ctx.quote_depth > 0 {
                        ctx.quote_pending = true;
                    }
                }
            }

            // ----- Emphasis (emission gated on features) -----
            Event::Start(Tag::Strong) if features.bold => ctx.write("**"),
            Event::End(TagEnd::Strong) if features.bold => ctx.write("**"),
            Event::Start(Tag::Emphasis) if features.italic => ctx.write("*"),
            Event::End(TagEnd::Emphasis) if features.italic => ctx.write("*"),
            Event::Start(Tag::Strikethrough) if features.strikethrough => ctx.write("~~"),
            Event::End(TagEnd::Strikethrough) if features.strikethrough => ctx.write("~~"),

            // ----- Headings -----
            Event::Start(Tag::Heading { level, .. }) if features.headings => {
                ctx.begin_line();
                ctx.output.push_str(&"#".repeat(level as usize));
                ctx.output.push(' ');
            }
            Event::End(TagEnd::Heading(_)) if features.headings => {
                ctx.output.push_str("\n\n");
                if ctx.quote_depth > 0 {
                    ctx.quote_pending = true;
                }
            }
            // Unsupported headings → bold fallback only when bold is
            // supported; otherwise plain text + blank line (no literal `**`).
            Event::Start(Tag::Heading { .. }) => {
                if features.bold {
                    ctx.write("**");
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if features.bold {
                    ctx.write("**");
                }
                ctx.output.push_str("\n\n");
                if ctx.quote_depth > 0 {
                    ctx.quote_pending = true;
                }
            }

            // ----- Paragraphs -----
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                ctx.output.push_str("\n\n");
                if ctx.quote_depth > 0 {
                    ctx.quote_pending = true;
                }
            }

            // ----- Code blocks -----
            Event::Start(Tag::CodeBlock(_)) => {
                ctx.in_code_block = true;
                if features.code_blocks {
                    // Pass through verbatim with fences (QQ renders these).
                    ctx.output.push_str("\n```\n");
                } else if !ctx.output.is_empty() && !ctx.output.ends_with('\n') {
                    // Plain text: no fence markers, keep the code content.
                    ctx.output.push('\n');
                }
            }
            Event::End(TagEnd::CodeBlock) if ctx.in_code_block => {
                ctx.in_code_block = false;
                if features.code_blocks {
                    // Avoid doubling a newline already present at the end of
                    // the code text.
                    if ctx.output.ends_with('\n') {
                        ctx.output.push_str("```\n\n");
                    } else {
                        ctx.output.push_str("\n```\n\n");
                    }
                } else {
                    if !ctx.output.ends_with('\n') {
                        ctx.output.push('\n');
                    }
                    ctx.output.push('\n');
                }
            }

            // ----- Lists -----
            // Markers `- ` / `N. ` are always emitted: a natural plain-text
            // convention, kept even when the list flags are false.
            Event::Start(Tag::List(None)) => ctx.list_stack.push(ListKind::Unordered),
            Event::Start(Tag::List(Some(start))) => {
                ctx.list_stack.push(ListKind::Ordered(start));
            }
            Event::End(TagEnd::List(_)) => {
                ctx.list_stack.pop();
                if ctx.list_stack.is_empty() {
                    ctx.output.push('\n');
                    if ctx.quote_depth > 0 {
                        ctx.quote_pending = true;
                    }
                }
            }
            Event::Start(Tag::Item) => {
                ctx.begin_line();
                ctx.indent();
                match ctx.list_stack.last_mut() {
                    Some(ListKind::Ordered(n)) => {
                        ctx.output.push_str(&format!("{}. ", n));
                        *n += 1;
                    }
                    _ => ctx.output.push_str("- "),
                }
            }
            Event::End(TagEnd::Item) => {
                if !ctx.output.ends_with('\n') {
                    ctx.output.push('\n');
                }
                if ctx.quote_depth > 0 {
                    ctx.quote_pending = true;
                }
            }

            // ----- Task list items → plain list items -----
            // The checked state arrives as a separate TaskListMarker event we
            // don't model yet, so `[ ]`/`[x]` is simply not emitted (the Item
            // handling above already writes `- `).

            // ----- Blockquotes -----
            Event::Start(Tag::BlockQuote(_)) if features.blockquotes => {
                ctx.quote_depth += 1;
                ctx.quote_pending = true;
            }
            Event::End(TagEnd::BlockQuote(_)) if features.blockquotes && ctx.quote_depth > 0 => {
                ctx.quote_depth -= 1;
                if ctx.quote_depth == 0 {
                    ctx.quote_pending = false;
                }
                ctx.output.push('\n');
            }
            // Unsupported blockquotes → plain paragraphs, no literal `> `.
            Event::Start(Tag::BlockQuote(_)) => {}
            Event::End(TagEnd::BlockQuote(_)) => {
                ctx.output.push('\n');
            }

            // ----- Links -----
            Event::Start(Tag::Link { dest_url, .. }) if features.links => {
                ctx.link_url = Some(dest_url.into_string());
                ctx.write("[");
            }
            Event::End(TagEnd::Link) if features.links => {
                if let Some(url) = ctx.link_url.take() {
                    ctx.write(&format!("]({})", url));
                }
            }
            // Unsupported links → capture the text, emit `text (url)` at the
            // closing tag (handled before the match). URL is information;
            // dropping it silently loses the payload on plain-text platforms.
            Event::Start(Tag::Link { dest_url, .. }) => {
                ctx.link_url = Some(dest_url.into_string());
                ctx.link_text = Some(String::new());
            }
            Event::End(TagEnd::Link) => {}

            // ----- Images -----
            Event::Start(Tag::Image { dest_url, .. }) => {
                ctx.image_url = Some(dest_url.into_string());
                if features.images {
                    ctx.write("![");
                } else {
                    // Fallback text also survives the WeChat SDK filter,
                    // which removes raw image syntax entirely.
                    ctx.write("[Image: ");
                }
            }
            Event::End(TagEnd::Image) => {
                if let Some(url) = ctx.image_url.take() {
                    if features.images {
                        ctx.write(&format!("]({})", url));
                    } else {
                        ctx.write("]");
                    }
                }
            }

            // ----- Horizontal rules → strip -----
            Event::Rule => {
                ctx.output.push('\n');
            }

            // ----- Tables -----
            Event::Start(Tag::Table(_)) if features.tables => {
                ctx.verbatim_table = Some(range.start);
            }
            Event::Start(Tag::Table(_)) => {
                ctx.in_table = true;
                ctx.table_rows.clear();
                ctx.current_row.clear();
            }
            Event::End(TagEnd::Table) if ctx.in_table => {
                ctx.in_table = false;
                let rendered = linearize_table(&ctx.table_rows);
                ctx.begin_line();
                ctx.output.push_str(&rendered);
                ctx.table_rows.clear();
            }
            Event::Start(Tag::TableHead) => {}
            Event::End(TagEnd::TableHead) => {
                ctx.table_rows.push(std::mem::take(&mut ctx.current_row));
            }
            Event::Start(Tag::TableRow) => {}
            Event::End(TagEnd::TableRow) => {
                ctx.table_rows.push(std::mem::take(&mut ctx.current_row));
            }
            Event::Start(Tag::TableCell) => {
                ctx.cell_buf.clear();
                ctx.in_cell = true;
            }
            // End(TableCell) is handled before the match (cell routing).

            // ----- Footnote / definition / everything else → ignore -----
            _ => {}
        }
    }

    // Collapse 3+ newlines to 2 for tidy output.
    let mut output = ctx.output;
    while output.contains("\n\n\n") {
        output = output.replace("\n\n\n", "\n\n");
    }
    output.trim_end().to_string() + "\n"
}

/// Conservative plain-text detection for the byte-exact fast path.
///
/// Every character that can participate in a Markdown construct under the
/// enabled options (tables, task lists, strikethrough; no smart punctuation)
/// is a trigger: `*_~` emphasis/rules, `` ` `` code, `[]!` links/images/
/// reference definitions, `|` tables, `>#` quotes/headings, `<` HTML/
/// autolinks, `&` entity references, `\` escapes, `-+` lists/rules, `.`
/// ordered lists, `=` setext headings. Indented code blocks (4 spaces / tab
/// at line start) need no trigger character and are checked separately.
///
/// Whitespace at line edges also changes bytes without any trigger char:
/// leading spaces are stripped from paragraph lines (and 4+ spaces form an
/// indented code block), trailing spaces are stripped at line ends (2+ form
/// a hard break). Lines with edge whitespace therefore take the slow path.
///
/// A false negative is impossible (missing a trigger would only mean parsing
/// text that did not need it); a false positive merely costs one parse.
fn is_plain_text(text: &str) -> bool {
    const TRIGGERS: &[char] = &[
        '*', '_', '~', '`', '[', ']', '!', '|', '>', '#', '<', '&', '\\', '-', '+', '.', '=',
    ];
    if text.chars().any(|c| TRIGGERS.contains(&c)) {
        return false;
    }
    !text.lines().any(|l| {
        l.starts_with(' ') || l.starts_with('\t') || l.ends_with(' ') || l.ends_with('\t')
    })
}

#[derive(Debug, Clone, Copy)]
enum ListKind {
    Unordered,
    Ordered(u64),
}

/// Projection state. Inline content is routed to one of three sinks:
/// the link-text capture (downgraded links), the current table cell, or the
/// main output — see [`RenderCtx::write`].
struct RenderCtx {
    output: String,
    in_code_block: bool,
    /// Blockquote nesting depth (supported quotes only).
    quote_depth: usize,
    /// A `> ` prefix is owed before the next output line (supported quotes).
    quote_pending: bool,
    list_stack: Vec<ListKind>,
    /// Pending URL of an open link (supported: emitted at the closing tag;
    /// downgraded: paired with the captured link text).
    link_url: Option<String>,
    /// Active link-text capture for downgraded links (`features.links=false`).
    link_text: Option<String>,
    image_url: Option<String>,
    // Table state
    in_table: bool,
    in_cell: bool,
    cell_buf: String,
    current_row: Vec<String>,
    table_rows: Vec<Vec<String>>,
    /// Source offset of a verbatim table pass-through (`features.tables=true`).
    verbatim_table: Option<usize>,
}

impl RenderCtx {
    fn new(capacity: usize) -> Self {
        Self {
            output: String::with_capacity(capacity),
            in_code_block: false,
            quote_depth: 0,
            quote_pending: false,
            list_stack: Vec::new(),
            link_url: None,
            link_text: None,
            image_url: None,
            in_table: false,
            in_cell: false,
            cell_buf: String::new(),
            current_row: Vec::new(),
            table_rows: Vec::new(),
            verbatim_table: None,
        }
    }

    /// Route inline content to the active sink: link-text capture first,
    /// then the table cell, then the main output (with blockquote prefix).
    fn write(&mut self, s: &str) {
        if let Some(link_text) = self.link_text.as_mut() {
            link_text.push_str(s);
        } else if self.in_cell {
            self.cell_buf.push_str(s);
        } else {
            self.begin_line();
            self.output.push_str(s);
        }
    }

    /// Emit the owed blockquote line prefix (`> ` per nesting level).
    fn begin_line(&mut self) {
        if self.quote_depth > 0 && self.quote_pending && !self.in_code_block {
            for _ in 0..self.quote_depth {
                self.output.push_str("> ");
            }
            self.quote_pending = false;
        }
    }

    fn indent(&mut self) {
        // Two spaces per nested list level (beyond the first).
        for _ in 0..self.list_stack.len().saturating_sub(1) {
            self.output.push_str("  ");
        }
    }
}

/// Linearize collected table rows into alignment-free plain text records.
///
/// `rows[0]` is the header; pulldown-cmark normalizes body rows to the header
/// width. Rules (deterministic, golden-tested):
///
/// - 1 column: each non-empty record cell on its own line.
/// - 2 columns (key/value): `key: value` per record; a lone key or value is
///   emitted alone; fully empty records are skipped; compact lines.
/// - N columns: first cell is the record head, the rest become `header: value`
///   pairs joined with ` · `, attached to the head with ` — `. Empty cells
///   skip their pair; an empty header name yields the bare value. Records are
///   separated by blank lines. Wide tables (>5 columns) put the head on its
///   own line and chunk pairs 3 per line — continuation lines are **not**
///   indented (CommonMark strips leading spaces on paragraph continuation,
///   which would break second-pass idempotency).
/// - Header-only tables carry no data and render as empty.
///
/// The output contains no pipe or separator syntax, so a second projection
/// pass is a no-op.
fn linearize_table(rows: &[Vec<String>]) -> String {
    if rows.len() < 2 {
        return String::new();
    }
    let header = &rows[0];
    let cols = header.len();
    if cols == 0 {
        return String::new();
    }
    fn cell(row: &[String], i: usize) -> &str {
        row.get(i).map(|s| s.as_str()).unwrap_or("")
    }

    let mut out = String::new();
    match cols {
        1 => {
            for row in &rows[1..] {
                let v = cell(row, 0);
                if !v.is_empty() {
                    out.push_str(v);
                    out.push('\n');
                }
            }
        }
        2 => {
            for row in &rows[1..] {
                let key = cell(row, 0);
                let val = cell(row, 1);
                match (key.is_empty(), val.is_empty()) {
                    (true, true) => continue,
                    (false, false) => {
                        out.push_str(key);
                        out.push_str(": ");
                        out.push_str(val);
                    }
                    (false, true) => out.push_str(key),
                    (true, false) => out.push_str(val),
                }
                out.push('\n');
            }
        }
        _ => {
            let wide = cols > 5;
            let mut first = true;
            for row in &rows[1..] {
                let head = cell(row, 0);
                let mut pairs: Vec<String> = Vec::new();
                for i in 1..cols {
                    let v = cell(row, i);
                    if v.is_empty() {
                        continue;
                    }
                    let h = cell(header, i);
                    if h.is_empty() {
                        pairs.push(v.to_string());
                    } else {
                        pairs.push(format!("{}: {}", h, v));
                    }
                }
                if head.is_empty() && pairs.is_empty() {
                    continue;
                }
                if !first {
                    out.push('\n'); // blank line between records
                }
                first = false;
                if wide {
                    if !head.is_empty() {
                        out.push_str(head);
                        out.push('\n');
                    }
                    for chunk in pairs.chunks(3) {
                        out.push_str(&chunk.join(" · "));
                        out.push('\n');
                    }
                } else {
                    if !head.is_empty() {
                        out.push_str(head);
                        if !pairs.is_empty() {
                            out.push_str(" — ");
                        }
                    }
                    out.push_str(&pairs.join(" · "));
                    out.push('\n');
                }
            }
        }
    }
    if !out.is_empty() {
        out.push('\n'); // blank line after the table block
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qq() -> MarkdownFeatures {
        MarkdownFeatures::qq()
    }

    /// All-false features = plain-text projection (WeChat).
    fn plain() -> MarkdownFeatures {
        MarkdownFeatures::default()
    }

    fn feishu() -> MarkdownFeatures {
        MarkdownFeatures::feishu()
    }

    // ------------------------------------------------------------------
    // Existing QQ regression (unchanged behavior except tables)
    // ------------------------------------------------------------------

    #[test]
    fn strips_bold_and_italic_for_qq() {
        // QQ does not support bold/italic markdown - they should be stripped to plain text.
        let out = prepare_markdown_for_platform("**bold** and *italic*", &qq());
        assert!(!out.contains("**bold**"));
        assert!(!out.contains("*italic*"));
        assert!(out.contains("bold and italic"));
    }

    #[test]
    fn passes_through_code_blocks_for_qq() {
        // QQ does support code blocks.
        let md = "```rust\nfn main() {}\n```\n";
        let out = prepare_markdown_for_platform(md, &qq());
        assert!(out.contains("```\nfn main() {}\n```"));
    }

    #[test]
    fn strips_inline_code_for_qq() {
        // QQ does not support inline code markdown - backticks should be removed.
        let out = prepare_markdown_for_platform("use `cargo` to build", &qq());
        assert!(!out.contains("`cargo`"));
        assert!(out.contains("cargo"));
    }

    #[test]
    fn strips_links_for_qq() {
        // QQ does not support link markdown - output plain text.
        let out = prepare_markdown_for_platform("[site](https://example.com)", &qq());
        // Links should be converted to plain text (either the label or URL, not markdown).
        assert!(out.contains("site") || out.contains("https://example.com"));
    }

    #[test]
    fn converts_image_to_alt_fallback() {
        let out = prepare_markdown_for_platform("![logo](https://x.com/a.png)", &qq());
        assert!(out.contains("[Image: logo]"));
        assert!(!out.contains("https://x.com/a.png"));
    }

    #[test]
    fn strips_html_tags() {
        let out = prepare_markdown_for_platform("<b>hi</b>", &qq());
        assert!(!out.contains("<b>"));
        assert!(out.contains("hi"));
    }

    #[test]
    fn preserves_code_block_contents_with_dashes() {
        // Dashes inside a code block must not be mangled into rules.
        let md = "```\n---\nx\n```\n";
        let out = prepare_markdown_for_platform(md, &qq());
        assert!(out.contains("---\nx"));
    }

    #[test]
    fn handles_empty_input() {
        let out = prepare_markdown_for_platform("", &qq());
        assert!(out.trim().is_empty());
    }

    #[test]
    fn handles_unicode_for_qq() {
        // Bold markers are stripped for QQ, but the text content should remain.
        let out = prepare_markdown_for_platform("**你好** 世界", &qq());
        assert!(out.contains("你好"));
        assert!(out.contains("世界"));
    }

    #[test]
    fn unsupported_headings_become_bold() {
        let f = MarkdownFeatures {
            bold: true,
            ..MarkdownFeatures::default() // headings disabled
        };
        let out = prepare_markdown_for_platform("# Title", &f);
        assert!(out.contains("**Title**"));
    }

    #[test]
    fn ordered_list_increments() {
        let f = MarkdownFeatures::default();
        let out = prepare_markdown_for_platform("1. first\n2. second\n3. third\n", &f);
        assert!(out.contains("1. first"), "expected '1. first' in: {out}");
        assert!(out.contains("2. second"), "expected '2. second' in: {out}");
        assert!(out.contains("3. third"), "expected '3. third' in: {out}");
    }

    // ------------------------------------------------------------------
    // T1: table linearization goldens
    // ------------------------------------------------------------------

    const STOCK_TABLE: &str = "| 持仓 | 现价 | 盈亏 | 仓位 |\n\
                               |---|---|---|---|\n\
                               | 贵州茅台 | 105.00 | -4.59% | 28.0% |\n\
                               | 五粮液 | 118.20 | -1.50% | 15.2% |\n";

    const STOCK_LINEAR: &str = "贵州茅台 — 现价: 105.00 · 盈亏: -4.59% · 仓位: 28.0%\n\
                                \n\
                                五粮液 — 现价: 118.20 · 盈亏: -1.50% · 仓位: 15.2%\n";

    #[test]
    fn linearize_n_column_golden() {
        // The real-world stock holdings answer: identical on QQ and WeChat.
        assert_eq!(prepare_markdown_for_platform(STOCK_TABLE, &qq()), STOCK_LINEAR);
        assert_eq!(prepare_markdown_for_platform(STOCK_TABLE, &plain()), STOCK_LINEAR);
    }

    #[test]
    fn linearize_two_column_golden() {
        let md = "| 指标 | 值 |\n|---|---|\n| 总资产 | 290362.30 |\n| 仓位 | 68.9% |\n";
        let out = prepare_markdown_for_platform(md, &plain());
        assert_eq!(out, "总资产: 290362.30\n仓位: 68.9%\n");
    }

    #[test]
    fn linearize_wide_table_golden() {
        // >5 columns: head on its own line, pairs chunked 3 per line,
        // continuation lines NOT indented (idempotency).
        let md = "| 持仓 | 现价 | 盈亏 | 仓位 | 今开 | 昨收 |\n\
                  |---|---|---|---|---|---|\n\
                  | 贵州茅台 | 105.00 | -4.59% | 28.0% | 104.00 | 109.99 |\n";
        let out = prepare_markdown_for_platform(md, &plain());
        assert_eq!(
            out,
            "贵州茅台\n现价: 105.00 · 盈亏: -4.59% · 仓位: 28.0%\n今开: 104.00 · 昨收: 109.99\n"
        );
    }

    #[test]
    fn linearize_skips_empty_cells() {
        let md = "| 持仓 | 现价 | 盈亏 | 仓位 |\n|---|---|---|---|\n| 贵州茅台 | 105.00 | | 28.0% |\n";
        let out = prepare_markdown_for_platform(md, &plain());
        assert_eq!(out, "贵州茅台 — 现价: 105.00 · 仓位: 28.0%\n");
        assert!(!out.contains("盈亏:"));
    }

    #[test]
    fn linearize_empty_record_head() {
        let md = "| 持仓 | 现价 | 盈亏 |\n|---|---|---|\n| | 105.00 | -4.59% |\n";
        let out = prepare_markdown_for_platform(md, &plain());
        assert_eq!(out, "现价: 105.00 · 盈亏: -4.59%\n");
    }

    #[test]
    fn linearize_empty_header_name() {
        let md = "| 持仓 | | 盈亏 |\n|---|---|---|\n| 茅台 | 105 | -4% |\n";
        let out = prepare_markdown_for_platform(md, &plain());
        assert_eq!(out, "茅台 — 105 · 盈亏: -4%\n");
    }

    #[test]
    fn linearize_two_column_lone_key_or_value() {
        let md = "| 指标 | 值 |\n|---|---|\n| 总资产 | |\n| | 68.9% |\n| | |\n";
        let out = prepare_markdown_for_platform(md, &plain());
        assert_eq!(out, "总资产\n68.9%\n");
    }

    #[test]
    fn linearize_single_column() {
        let md = "| 项目 |\n|---|\n| 甲 |\n| 乙 |\n";
        let out = prepare_markdown_for_platform(md, &plain());
        assert_eq!(out, "甲\n乙\n");
    }

    #[test]
    fn linearize_header_only_is_empty() {
        let md = "| a | b |\n|---|---|\n";
        let out = prepare_markdown_for_platform(md, &plain());
        assert!(out.trim().is_empty());
    }

    #[test]
    fn linearize_short_row_normalized_to_header_width() {
        // pulldown-cmark pads short body rows to the header column count.
        let md = "| a | b | c |\n|---|---|---|\n| 1 |\n";
        let out = prepare_markdown_for_platform(md, &plain());
        assert_eq!(out, "1\n");
    }

    #[test]
    fn linearize_no_table_syntax_residue() {
        for out in [
            prepare_markdown_for_platform(STOCK_TABLE, &plain()),
            prepare_markdown_for_platform("| a | b |\n|---|---|\n| 1 | 2 |\n", &qq()),
        ] {
            assert!(!out.contains('|'), "pipe residue in: {out}");
            assert!(!out.contains("---"), "separator residue in: {out}");
            assert!(!out.contains("-+-"), "separator residue in: {out}");
        }
    }

    #[test]
    fn linearize_idempotent() {
        let inputs = [
            STOCK_TABLE,
            "| 指标 | 值 |\n|---|---|\n| 总资产 | 290362.30 |\n| 仓位 | 68.9% |\n",
            "| 持仓 | 现价 | 盈亏 | 仓位 | 今开 | 昨收 |\n|---|---|---|---|---|---|\n| 贵州茅台 | 105.00 | -4.59% | 28.0% | 104.00 | 109.99 |\n",
            "| 持仓 | | 盈亏 |\n|---|---|---|\n| 茅台 | 105 | -4% |\n",
            "| 项目 |\n|---|\n| 甲 |\n| 乙 |\n",
        ];
        for features in [qq(), plain()] {
            for md in inputs {
                let once = prepare_markdown_for_platform(md, &features);
                let twice = prepare_markdown_for_platform(&once, &features);
                assert_eq!(once, twice, "second pass changed bytes for: {md}");
            }
        }
    }

    #[test]
    fn tables_true_passes_through_verbatim() {
        // Escaped pipes and inline syntax survive byte-level (source slice).
        let md = "| **a** | b |\n|---|---|\n| 1 | x\\|y |\n";
        let out = prepare_markdown_for_platform(md, &feishu());
        assert!(out.contains("| **a** | b |"));
        assert!(out.contains("x\\|y"));
        assert!(out.contains("|---|---|"));
        // Idempotent: the verbatim table is sliced again unchanged.
        let twice = prepare_markdown_for_platform(&out, &feishu());
        assert_eq!(out, twice);
    }

    // ------------------------------------------------------------------
    // T2: feature-respecting downgrades (plain-text projection)
    // ------------------------------------------------------------------

    #[test]
    fn plain_text_projection_has_no_markdown_residue() {
        let md = "# 标题\n\n**加粗** 与 ~~删除~~\n\n```rust\nfn main() {}\n```\n\n\
                  > 引用文字\n\n[站点](https://example.com)\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let out = prepare_markdown_for_platform(md, &plain());
        assert_eq!(
            out,
            "标题\n\n加粗 与 删除\n\nfn main() {}\n\n引用文字\n\n站点 (https://example.com)\n\n1: 2\n"
        );
        // List markers are the only allowed convention; nothing else remains.
        for marker in ["**", "~~", "```", "# ", "> ", "|", "](", "!["] {
            assert!(!out.contains(marker), "marker {marker:?} leaked into: {out}");
        }
    }

    #[test]
    fn qq_heading_downgrades_to_plain_text() {
        // QQ: headings=false, bold=false → no literal `**` (behavior change,
        // pinned deliberately: caps honesty over marker residue).
        let out = prepare_markdown_for_platform("# 标题", &qq());
        assert_eq!(out, "标题\n");
    }

    #[test]
    fn qq_blockquote_has_no_literal_marker() {
        let out = prepare_markdown_for_platform("> 引用文字", &qq());
        assert_eq!(out, "引用文字\n");
    }

    #[test]
    fn qq_strikethrough_markers_stripped() {
        let out = prepare_markdown_for_platform("~~删除~~", &qq());
        assert_eq!(out, "删除\n");
    }

    #[test]
    fn code_blocks_false_strips_fences_keeps_content() {
        let md = "前文\n\n```\nlet x = 1;\n```\n\n后文";
        let out = prepare_markdown_for_platform(md, &plain());
        assert_eq!(out, "前文\n\nlet x = 1;\n\n后文\n");
        // Code at the very start gains no leading blank line.
        let out2 = prepare_markdown_for_platform("```\nlet x = 1;\n```\n", &plain());
        assert_eq!(out2, "let x = 1;\n");
    }

    #[test]
    fn links_false_keep_url_as_text_paren() {
        let out = prepare_markdown_for_platform("[站点](https://example.com)", &qq());
        assert_eq!(out, "站点 (https://example.com)\n");
        // text == url → no duplication.
        let out = prepare_markdown_for_platform(
            "[https://example.com](https://example.com)",
            &qq(),
        );
        assert_eq!(out, "https://example.com\n");
        // Empty text → bare URL.
        let out = prepare_markdown_for_platform("[](https://example.com)", &qq());
        assert_eq!(out, "https://example.com\n");
    }

    #[test]
    fn blockquotes_true_prefix_every_line() {
        // F1 fix: supported quotes keep their markers on every line.
        let out = prepare_markdown_for_platform("> 第一行\n> 第二行\n", &feishu());
        assert_eq!(out, "> 第一行\n> 第二行\n");
        // Nested quotes: pulldown ends the outer paragraph before the nested
        // quote, and each nesting level contributes one `> ` prefix.
        let out = prepare_markdown_for_platform("> 外\n> > 内\n", &feishu());
        assert_eq!(out, "> 外\n\n> > 内\n");
        // Idempotent.
        let once = prepare_markdown_for_platform("> 第一行\n> 第二行\n", &feishu());
        assert_eq!(once, prepare_markdown_for_platform(&once, &feishu()));
    }

    #[test]
    fn images_true_passes_through() {
        let out = prepare_markdown_for_platform("![logo](https://x.com/a.png)", &feishu());
        assert_eq!(out, "![logo](https://x.com/a.png)\n");
    }

    #[test]
    fn hard_break_is_plain_newline() {
        let out = prepare_markdown_for_platform("a  \nb", &qq());
        assert_eq!(out, "a\nb\n");
        assert!(!out.contains("  \n"));
    }

    #[test]
    fn cell_captures_inline_projection() {
        // Inline code / bold inside cells follow the platform features.
        // (tables stays false here so the table is linearized, not verbatim.)
        let mut rich = plain();
        rich.inline_code = true;
        rich.bold = true;
        let md = "| 键 | 值 |\n|---|---|\n| `code` | **bold** |\n";
        assert_eq!(prepare_markdown_for_platform(md, &rich), "`code`: **bold**\n");
        assert_eq!(prepare_markdown_for_platform(md, &qq()), "code: bold\n");
    }

    #[test]
    fn cell_link_downgrade_keeps_url() {
        let md = "| 来源 | 链接 |\n|---|---|\n| 官网 | [站点](https://example.com) |\n";
        let out = prepare_markdown_for_platform(md, &plain());
        assert_eq!(out, "官网: 站点 (https://example.com)\n");
    }

    // ------------------------------------------------------------------
    // Plain-text fast path
    // ------------------------------------------------------------------

    #[test]
    fn fast_path_is_byte_exact() {
        // No triggers and no edge whitespace: pass through untouched,
        // including the trailing newline (no normalization).
        let text = "你好世界 👋\n第二行\n";
        assert_eq!(prepare_markdown_for_platform(text, &plain()), text);
        assert_eq!(prepare_markdown_for_platform(text, &qq()), text);
        let text = "总资产 290362 元，仓位 68%";
        assert_eq!(prepare_markdown_for_platform(text, &plain()), text);
        // Edge whitespace is a Markdown construct (hard break / stripped
        // paragraph whitespace) and must NOT take the fast path.
        let out = prepare_markdown_for_platform("尾随空格  \n下一行", &plain());
        assert_eq!(out, "尾随空格\n下一行\n");
    }

    #[test]
    fn fast_path_excludes_indented_code() {
        // 4-space indent forms a code block without any trigger character.
        let out = prepare_markdown_for_platform("    code\n", &plain());
        assert_eq!(out, "code\n");
        let out = prepare_markdown_for_platform("    code\n", &qq());
        assert!(out.contains("```"));
        assert!(out.contains("code"));
    }

    #[test]
    fn fast_path_idempotent_on_projected_plain_output() {
        // Linearized records without triggers hit the fast path on pass 2.
        let once = prepare_markdown_for_platform("| 项目 |\n|---|\n| 甲 |\n| 乙 |\n", &plain());
        assert_eq!(once, "甲\n乙\n");
        assert_eq!(prepare_markdown_for_platform(&once, &plain()), once);
    }
}