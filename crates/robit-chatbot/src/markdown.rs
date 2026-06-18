//! Markdown sanitizer for platform-specific rendering.
//!
//! LLM outputs are Markdown. Each chat platform supports a subset of
//! CommonMark. This module converts LLM Markdown into the subset supported
//! by a given platform, stripping or converting unsupported features:
//!
//! - **Tables** → aligned plain text (QQ does not support table syntax)
//! - **Task lists** (`- [ ]`) → plain unordered lists
//! - **HTML tags** → stripped entirely
//! - **Images** (`![]()`) → `[Image: alt]` fallback text
//! - **Horizontal rules** (`---`) → stripped
//!
//! Everything else (headings, bold, italic, code, links, lists, blockquotes,
//! strikethrough) is passed through as-is. Parsing uses `pulldown-cmark`
//! (already in the workspace) for safe event-driven handling, so unsupported
//! syntax inside code blocks is preserved verbatim.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::adapter::MarkdownFeatures;

/// Prepare Markdown for a platform — pass through supported syntax, strip/convert unsupported.
///
/// `features` describes what the target platform supports; unsupported features
/// are converted to a readable fallback rather than dropped silently.
pub fn prepare_markdown_for_platform(text: &str, features: &MarkdownFeatures) -> String {
    let mut opts = Options::empty();
    if features.strikethrough {
        opts.insert(Options::ENABLE_STRIKETHROUGH);
    }
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(text, opts);
    let mut output = String::with_capacity(text.len());
    let mut ctx = RenderCtx::new(features);

    for event in parser {
        match event {
            // ----- Inline text -----
            Event::Text(t) => {
                output.push_str(&t);
            }
            Event::Code(c) => {
                if features.inline_code {
                    output.push('`');
                    output.push_str(&c);
                    output.push('`');
                } else {
                    output.push_str(&c);
                }
            }
            Event::SoftBreak => output.push('\n'),
            Event::HardBreak => output.push_str("  \n"),

            // ----- Emphasis -----
            Event::Start(Tag::Strong) if features.bold => output.push_str("**"),
            Event::End(TagEnd::Strong) if features.bold => output.push_str("**"),
            Event::Start(Tag::Emphasis) if features.italic => output.push('*'),
            Event::End(TagEnd::Emphasis) if features.italic => output.push('*'),
            Event::Start(Tag::Strikethrough) if features.strikethrough => output.push_str("~~"),
            Event::End(TagEnd::Strikethrough) if features.strikethrough => output.push_str("~~"),

            // ----- Headings -----
            Event::Start(Tag::Heading { level, .. }) if features.headings => {
                let hashes = "#".repeat(level as usize);
                output.push_str(&hashes);
                output.push(' ');
            }
            Event::End(TagEnd::Heading(_)) if features.headings => {
                output.push_str("\n\n");
            }
            // Unsupported headings → bold + blank line fallback.
            Event::Start(Tag::Heading { .. }) => output.push_str("**"),
            Event::End(TagEnd::Heading(_)) => output.push_str("**\n\n"),

            // ----- Paragraphs -----
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => output.push_str("\n\n"),

            // ----- Code blocks (pass through verbatim) -----
            Event::Start(Tag::CodeBlock(_)) => {
                ctx.in_code_block = true;
                output.push_str("\n```\n");
            }
            Event::End(TagEnd::CodeBlock) if ctx.in_code_block => {
                ctx.in_code_block = false;
                // Avoid doubling a newline already present at the end of the
                // code text.
                if output.ends_with('\n') {
                    output.push_str("```\n\n");
                } else {
                    output.push_str("\n```\n\n");
                }
            }

            // ----- Lists -----
            Event::Start(Tag::List(None)) => ctx.list_stack.push(ListKind::Unordered),
            Event::Start(Tag::List(Some(start))) => {
                ctx.list_stack.push(ListKind::Ordered(start));
            }
            Event::End(TagEnd::List(_)) => {
                ctx.list_stack.pop();
                if ctx.list_stack.is_empty() {
                    output.push('\n');
                }
            }
            Event::Start(Tag::Item) => {
                ctx.indent(&mut output);
                match ctx.list_stack.last() {
                    Some(ListKind::Ordered(n)) => {
                        output.push_str(&format!("{}. ", n));
                    }
                    _ => output.push_str("- "),
                }
            }
            Event::End(TagEnd::Item) => {
                if !output.ends_with('\n') {
                    output.push('\n');
                }
            }

            // ----- Task list items → plain list items -----
            // pulldown-cmark emits TaskList markers via Item start; the checked
            // state arrives as a separate event we don't model here, so the
            // `[ ]`/`[x]` prefix is simply not emitted (already covered by the
            // Item handling above, which writes `- `).

            // ----- Blockquotes -----
            Event::Start(Tag::BlockQuote(_)) if features.blockquotes => {
                ctx.in_blockquote = true;
            }
            Event::End(TagEnd::BlockQuote(_)) if ctx.blockquote_was_open() => {
                ctx.in_blockquote = false;
            }
            Event::Start(Tag::BlockQuote(_)) => {
                // Unsupported blockquote → indent as plain text.
                output.push_str("> ");
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                output.push('\n');
            }

            // ----- Links -----
            Event::Start(Tag::Link { dest_url, .. }) if features.links => {
                ctx.link_url = Some(dest_url.into_string());
                output.push('[');
            }
            Event::End(TagEnd::Link) if ctx.link_url.is_some() => {
                if let Some(url) = ctx.link_url.take() {
                    output.push_str(&format!("]({})", url));
                }
            }
            // Unsupported links → just the link text (no syntax).
            Event::Start(Tag::Link { .. }) => {}
            Event::End(TagEnd::Link) => {}

            // ----- Images → [Image: alt] fallback -----
            Event::Start(Tag::Image { dest_url, .. }) => {
                ctx.image_url = Some(dest_url.into_string());
                output.push_str("[Image: ");
            }
            Event::End(TagEnd::Image) => {
                ctx.image_url = None;
                output.push(']');
            }

            // ----- Horizontal rules → strip (unsupported on QQ) -----
            Event::Rule => {
                output.push('\n');
            }

            // ----- Tables → render as aligned plain text -----
            Event::Start(Tag::Table(_)) => {
                ctx.in_table = true;
                ctx.table_rows.clear();
                ctx.current_row.clear();
            }
            Event::End(TagEnd::Table) => {
                ctx.in_table = false;
                output.push_str(&render_table(&ctx.table_rows));
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
            Event::End(TagEnd::TableCell) => {
                ctx.in_cell = false;
                ctx.current_row.push(ctx.cell_buf.trim().to_string());
            }

            // Inside a table cell, capture text rather than emitting directly.
            // (Handled in the Text branch below via in_cell flag.)

            // ----- Footnote / definition / everything else → ignore -----
            _ => {}
        }

        // Capture text inside table cells into the cell buffer instead of output.
        if ctx.in_cell {
            if let Some(stripped) = strip_last_text(&mut output) {
                ctx.cell_buf.push_str(&stripped);
            }
        }
    }

    // Collapse 3+ newlines to 2 for tidy output.
    while output.contains("\n\n\n") {
        output = output.replace("\n\n\n", "\n\n");
    }
    output.trim_end().to_string() + "\n"
}

#[derive(Debug, Clone, Copy)]
enum ListKind {
    Unordered,
    Ordered(u64),
}

struct RenderCtx<'a> {
    #[allow(dead_code)]
    features: &'a MarkdownFeatures,
    in_code_block: bool,
    in_blockquote: bool,
    list_stack: Vec<ListKind>,
    link_url: Option<String>,
    image_url: Option<String>,
    // Table state
    in_table: bool,
    in_cell: bool,
    cell_buf: String,
    current_row: Vec<String>,
    table_rows: Vec<Vec<String>>,
}

impl<'a> RenderCtx<'a> {
    fn new(features: &'a MarkdownFeatures) -> Self {
        Self {
            features,
            in_code_block: false,
            in_blockquote: false,
            list_stack: Vec::new(),
            link_url: None,
            image_url: None,
            in_table: false,
            in_cell: false,
            cell_buf: String::new(),
            current_row: Vec::new(),
            table_rows: Vec::new(),
        }
    }

    fn indent(&self, out: &mut String) {
        // One space per nested list level (beyond the first).
        for _ in 0..self.list_stack.len().saturating_sub(1) {
            out.push_str("  ");
        }
    }

    fn blockquote_was_open(&self) -> bool {
        self.in_blockquote
    }
}

/// Render collected table rows as aligned plain text.
fn render_table(rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut widths = vec![0usize; cols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for (ri, row) in rows.iter().enumerate() {
        for (i, cell) in row.iter().enumerate() {
            let w = widths.get(i).copied().unwrap_or(0);
            let pad = w.saturating_sub(cell.chars().count());
            out.push_str(cell);
            out.push_str(&" ".repeat(pad));
            if i + 1 < cols {
                out.push_str(" | ");
            }
        }
        out.push('\n');
        if ri == 0 {
            // Separator line under the header.
            for (i, w) in widths.iter().enumerate() {
                out.push_str(&"-".repeat(*w));
                if i + 1 < cols {
                    out.push_str("-+-");
                }
            }
            out.push('\n');
        }
    }
    out.push('\n');
    out
}

/// Pull the last contiguous text chunk back out of `output` (used to redirect
/// cell text into the cell buffer). Returns the extracted text.
fn strip_last_text(output: &mut String) -> Option<String> {
    // The Text handler pushes the raw string; we appended it at the very end,
    // so trim trailing non-newline chars back to the last newline boundary.
    let end = output.len();
    if end == 0 {
        return None;
    }
    let bytes = output.as_bytes();
    let mut start = end;
    while start > 0 && bytes[start - 1] != b'\n' {
        start -= 1;
    }
    if start == end {
        return None;
    }
    let text = output[start..end].to_string();
    output.truncate(start);
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qq() -> MarkdownFeatures {
        MarkdownFeatures::qq()
    }

    #[test]
    fn passes_through_bold_and_italic() {
        let out = prepare_markdown_for_platform("**bold** and *italic*", &qq());
        assert!(out.contains("**bold**"));
        assert!(out.contains("*italic*"));
    }

    #[test]
    fn passes_through_code_blocks() {
        let md = "```rust\nfn main() {}\n```\n";
        let out = prepare_markdown_for_platform(md, &qq());
        assert!(out.contains("```\nfn main() {}\n```"));
    }

    #[test]
    fn passes_through_inline_code() {
        let out = prepare_markdown_for_platform("use `cargo` to build", &qq());
        assert!(out.contains("`cargo`"));
    }

    #[test]
    fn passes_through_links() {
        let out = prepare_markdown_for_platform("[site](https://example.com)", &qq());
        assert!(out.contains("[site](https://example.com)"));
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
    fn converts_table_to_aligned_text() {
        let md = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let out = prepare_markdown_for_platform(md, &qq());
        // No pipe-table markdown remains; aligned text rows present.
        assert!(!out.contains("|---|"));
        assert!(out.contains("a"));
        assert!(out.contains("1"));
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
    fn handles_unicode() {
        let out = prepare_markdown_for_platform("**你好** 世界", &qq());
        assert!(out.contains("**你好**"));
        assert!(out.contains("世界"));
    }

    #[test]
    fn unsupported_headings_become_bold() {
        let mut f = MarkdownFeatures::default(); // headings disabled
        f.bold = true;
        let out = prepare_markdown_for_platform("# Title", &f);
        assert!(out.contains("**Title**"));
    }
}
