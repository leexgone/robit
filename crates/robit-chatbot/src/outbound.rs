//! Outbound length guard for platform adapters.
//!
//! This module provides a **transparent** length enforcement layer that any
//! [`PlatformAdapter`](crate::adapter::PlatformAdapter) can call in its
//! `send_message` / `send_message_best_effort` implementation.  It guarantees
//! that text leaving the adapter respects the platform's declared
//! [`PlatformCaps::max_message_length`](crate::adapter::PlatformCaps::max_message_length),
//! regardless of whether the caller is the chatbot reply path or a host's
//! direct push.
//!
//! # Design
//!
//! - **Non-edit platforms** (e.g. WeChat): long text is split into at most
//!   [`MAX_OUTBOUND_SEGMENTS`] chunks using boundary-aware heuristics
//!   (paragraph > line > sentence > whitespace > hard cut).  The last segment
//!   carries [`TRUNCATION_MARKER`] when content had to be dropped.
//! - **Edit platforms** (e.g. QQ): long text is truncated to the cap with an
//!   ellipsis, preserving the single-message editing model.
//! - **Idempotent**: text already within the cap passes through unchanged
//!   (single-element vector, no allocation beyond the clone).  This means the
//!   chatbot reply path — which already segments in `flush_buffer` — is
//!   unaffected when its output flows through this guard again at the adapter
//!   level.
//!
//! # Partial-failure semantics
//!
//! When a multi-segment send fails partway, the recommended strategy is to
//! **abandon remaining segments** (continuing would only increase throttle
//! pressure with diminishing value).  The returned
//! [`OutboundResult::segments_sent`] field tells the caller how many segments
//! were successfully delivered so it can log accurately.

use crate::adapter::PlatformCaps;

/// Hard cap on how many messages one outbound call may produce on a platform
/// without in-place edit.
///
/// Editing platforms merge segments into one message, so they are unaffected;
/// without editing, an unbounded split would turn one long answer into a
/// message storm.  Anything beyond this cap is dropped **with an explicit
/// marker** (see [`TRUNCATION_MARKER`]) rather than silently.
pub const MAX_OUTBOUND_SEGMENTS: usize = 3;

/// Appended to the last segment when the reply had to be cut.
pub const TRUNCATION_MARKER: &str = "\n……（内容过长，已截断）";

/// Outcome of preparing outbound text through the length guard.
#[derive(Debug, Clone)]
pub struct OutboundResult {
    /// The segments to send, in order.  Always non-empty when the input is
    /// non-empty.  Each segment's char count is ≤ `caps.max_message_length`.
    pub segments: Vec<String>,
    /// Whether content was dropped (the last segment carries
    /// [`TRUNCATION_MARKER`]).
    pub truncated: bool,
}

/// Prepare `text` for sending on a platform described by `caps`.
///
/// - If `text` fits within `caps.max_message_length`, returns it as-is in a
///   single-element vector (idempotent pass-through).
/// - On **edit** platforms (`caps.supports_edit == true`), truncates to the
///   cap with an ellipsis — the single-message editing model must not be
///   broken by splitting.
/// - On **non-edit** platforms, splits into at most [`MAX_OUTBOUND_SEGMENTS`]
///   boundary-aware segments.  Content beyond the budget is marked with
///   [`TRUNCATION_MARKER`].
///
/// # Panics
///
/// Does not panic.  A zero `max_message_length` is treated as "no limit" and
/// the text is returned unchanged.
pub fn prepare_outbound(text: &str, caps: &PlatformCaps) -> OutboundResult {
    let max_len = caps.max_message_length;

    // No cap or empty text → pass through.
    if max_len == 0 || text.is_empty() {
        return OutboundResult {
            segments: vec![text.to_string()],
            truncated: false,
        };
    }

    let total = text.chars().count();
    if total <= max_len {
        return OutboundResult {
            segments: vec![text.to_string()],
            truncated: false,
        };
    }

    if caps.supports_edit {
        // Edit platform: truncate, do not split.
        OutboundResult {
            segments: vec![truncate_to_max(text, max_len)],
            truncated: true,
        }
    } else {
        // Non-edit platform: split into bounded segments.
        let (segments, truncated) =
            split_for_platform(text, max_len, MAX_OUTBOUND_SEGMENTS);
        OutboundResult {
            segments,
            truncated,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers (same logic as the original private fns in frontend.rs)
// ---------------------------------------------------------------------------

/// Truncate `text` to `max` characters, appending an ellipsis if cut.
fn truncate_to_max(text: &str, max: usize) -> String {
    if max == 0 {
        return text.to_string();
    }
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Split `text` into at most `max_segments` chunks of at most `max_len` chars.
///
/// Boundaries are preferred in this order: paragraph, line, sentence end,
/// whitespace, hard cut.  A boundary inside a fenced code block is skipped
/// when another usable boundary exists, so fenced code survives intact.
///
/// Returns the segments plus whether content had to be dropped (the last
/// segment then carries [`TRUNCATION_MARKER`]).
fn split_for_platform(text: &str, max_len: usize, max_segments: usize) -> (Vec<String>, bool) {
    let total = text.chars().count();
    if total == 0 {
        return (Vec::new(), false);
    }
    if max_len == 0 || max_segments == 0 || total <= max_len {
        return (vec![text.to_string()], false);
    }

    let marker_chars = TRUNCATION_MARKER.chars().count();
    let mut segments: Vec<String> = Vec::new();
    let mut rest = text;

    loop {
        let rest_chars = rest.chars().count();
        if rest_chars <= max_len {
            segments.push(rest.to_string());
            return (segments, false);
        }

        if segments.len() + 1 >= max_segments {
            // Last allowed segment: keep what fits next to the marker.
            let budget = max_len.saturating_sub(marker_chars);
            let (head, _tail) = split_head(rest, budget.max(1));
            segments.push(format!("{}{}", head.trim_end(), TRUNCATION_MARKER));
            return (segments, true);
        }

        let (head, tail) = split_head(rest, max_len);
        segments.push(head.to_string());
        rest = tail;
    }
}

/// Cut `text` into `(head, tail)`, with at most `max_chars` chars in the head.
fn split_head(text: &str, max_chars: usize) -> (&str, &str) {
    let hard_cut = byte_index_of_char(text, max_chars);
    if hard_cut == 0 {
        return ("", text);
    }
    let window = &text[..hard_cut];
    // Do not cut far too early just because a newline happens to sit at the
    // very beginning of the window (e.g. a short lead-in paragraph).
    let min_chars = (max_chars / 3).max(1);

    for candidate in boundary_candidates(window) {
        if candidate == 0 || candidate > hard_cut {
            continue;
        }
        if text[..candidate].chars().count() < min_chars {
            continue;
        }
        // `candidate` may sit inside a fenced code block: prefer another one.
        if inside_code_fence(&text[..candidate]) {
            continue;
        }
        return text.split_at(candidate);
    }
    text.split_at(hard_cut)
}

/// Byte index of the `max_chars`-th char (i.e. where a hard cut would land).
fn byte_index_of_char(text: &str, max_chars: usize) -> usize {
    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => idx,
        None => text.len(),
    }
}

/// Candidate split positions (byte indices), most preferred first.
fn boundary_candidates(window: &str) -> Vec<usize> {
    let mut candidates = Vec::new();
    if let Some(idx) = window.rfind("\n\n") {
        candidates.push(idx + 2); // keep the blank line with the head
    }
    if let Some(idx) = window.rfind('\n') {
        candidates.push(idx + 1);
    }
    for punct in ["。", "！", "？", "；", ".", "!", "?", ";"] {
        if let Some(idx) = window.rfind(punct) {
            candidates.push(idx + punct.len());
        }
    }
    if let Some(idx) = window.rfind(' ') {
        candidates.push(idx + 1);
    }
    candidates
}

/// Whether `prefix` ends inside a fenced code block (` ``` ` pairs).
fn inside_code_fence(prefix: &str) -> bool {
    prefix.matches("```").count() % 2 == 1
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{HintPolicy, MarkdownFeatures, PlatformCaps};

    fn non_edit_caps(max_len: usize) -> PlatformCaps {
        PlatformCaps {
            supports_edit: false,
            returns_msg_id: true,
            supports_markdown: false,
            markdown_features: MarkdownFeatures::wechat(),
            max_message_length: max_len,
            supports_images: true,
            supports_files: true,
            max_upload_size: 20 * 1024 * 1024,
            progress_hints: HintPolicy::Off,
        }
    }

    fn edit_caps(max_len: usize) -> PlatformCaps {
        PlatformCaps {
            supports_edit: true,
            returns_msg_id: true,
            supports_markdown: true,
            markdown_features: MarkdownFeatures::qq(),
            max_message_length: max_len,
            supports_images: true,
            supports_files: true,
            max_upload_size: 20 * 1024 * 1024,
            progress_hints: HintPolicy::Always,
        }
    }

    // -- Idempotent pass-through ------------------------------------------

    #[test]
    fn short_text_passes_through_unchanged() {
        let caps = non_edit_caps(2000);
        let result = prepare_outbound("你好世界", &caps);
        assert_eq!(result.segments, vec!["你好世界".to_string()]);
        assert!(!result.truncated);
    }

    #[test]
    fn exact_cap_text_passes_through() {
        let caps = non_edit_caps(100);
        let text = "A".repeat(100);
        let result = prepare_outbound(&text, &caps);
        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.segments[0], text);
        assert!(!result.truncated);
    }

    #[test]
    fn empty_text_returns_single_empty_segment() {
        let caps = non_edit_caps(2000);
        let result = prepare_outbound("", &caps);
        assert_eq!(result.segments, vec!["".to_string()]);
        assert!(!result.truncated);
    }

    #[test]
    fn zero_cap_means_no_limit() {
        let caps = non_edit_caps(0);
        let text = "A".repeat(10000);
        let result = prepare_outbound(&text, &caps);
        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.segments[0].len(), 10000);
        assert!(!result.truncated);
    }

    // -- Non-edit platform: segmentation ----------------------------------

    #[test]
    fn non_edit_splits_long_text_into_bounded_segments() {
        let caps = non_edit_caps(100);
        let text = "X".repeat(700);
        let result = prepare_outbound(&text, &caps);
        assert!(result.truncated);
        assert_eq!(result.segments.len(), MAX_OUTBOUND_SEGMENTS);
        assert!(result.segments.last().unwrap().ends_with("已截断）"));
        assert!(result.segments.iter().all(|s| s.chars().count() <= 100));
    }

    #[test]
    fn non_edit_prefers_paragraph_boundaries() {
        let caps = non_edit_caps(200);
        let text = format!("{}\n\n{}", "A".repeat(120), "B".repeat(120));
        let result = prepare_outbound(&text, &caps);
        assert!(!result.truncated);
        assert_eq!(result.segments.len(), 2);
        assert!(result.segments[0].ends_with("\n\n"));
        assert_eq!(result.segments.concat(), text);
    }

    #[test]
    fn non_edit_does_not_cut_inside_code_fence() {
        let caps = non_edit_caps(220);
        let text = format!("```\n{}\n```\n\n{}", "x".repeat(170), "z".repeat(100));
        let result = prepare_outbound(&text, &caps);
        assert!(
            result.segments[0].contains("```"),
            "fence stays in the first segment"
        );
        assert!(result.segments[0].ends_with("\n\n"));
    }

    #[test]
    fn non_edit_hard_cuts_unbroken_text() {
        let caps = non_edit_caps(200);
        let text = "A".repeat(500);
        let result = prepare_outbound(&text, &caps);
        assert_eq!(result.segments[0].chars().count(), 200);
        // All original content preserved across segments (minus truncation marker).
        let concat: String = result.segments.iter()
            .map(|s| s.replace(TRUNCATION_MARKER, ""))
            .collect();
        assert_eq!(concat, text);
    }

    // -- Edit platform: truncation ----------------------------------------

    #[test]
    fn edit_platform_truncates_instead_of_splitting() {
        let caps = edit_caps(100);
        let text = "A".repeat(500);
        let result = prepare_outbound(&text, &caps);
        assert_eq!(result.segments.len(), 1);
        assert!(result.truncated);
        assert!(result.segments[0].chars().count() <= 100);
        assert!(result.segments[0].ends_with('…'));
    }

    #[test]
    fn edit_platform_short_text_untouched() {
        let caps = edit_caps(2000);
        let result = prepare_outbound("短消息", &caps);
        assert_eq!(result.segments, vec!["短消息".to_string()]);
        assert!(!result.truncated);
    }

    // -- Idempotency: pre-segmented text passes through -------------------

    #[test]
    fn pre_segmented_text_is_idempotent() {
        // Simulate what flush_buffer produces: each segment already ≤ cap.
        let caps = non_edit_caps(2000);
        let segment = "A".repeat(2000);
        let result = prepare_outbound(&segment, &caps);
        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.segments[0], segment);
        assert!(!result.truncated);
    }
}
