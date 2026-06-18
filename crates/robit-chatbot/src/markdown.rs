//! Markdown sanitizer for platform-specific rendering.
//!
//! Converts LLM Markdown output to a subset supported by the target platform,
//! stripping or converting unsupported features. Uses `pulldown-cmark` for
//! safe event-driven parsing.
//!
//! NOTE: Full implementation lands in Phase 4.

use crate::adapter::MarkdownFeatures;

/// Prepare Markdown for a platform — pass through supported syntax, strip/convert unsupported.
pub fn prepare_markdown_for_platform(text: &str, _features: &MarkdownFeatures) -> String {
    // Placeholder: pass through unchanged until Phase 4 implements the sanitizer.
    text.to_string()
}
