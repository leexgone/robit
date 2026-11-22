//! Outbound content projection + length guard for platform adapters.
//!
//! This module provides a **transparent** outbound preparation layer that any
//! [`PlatformAdapter`](crate::adapter::PlatformAdapter) can call in its
//! `send_message` / `send_message_best_effort` implementation.  It guarantees
//! that text leaving the adapter is both **platform-readable** (projected to
//! the platform's [`MarkdownFeatures`](crate::adapter::MarkdownFeatures)) and
//! **within the declared length cap**
//! ([`PlatformCaps::max_message_length`](crate::adapter::PlatformCaps::max_message_length)),
//! regardless of whether the caller is the chatbot reply path or a host's
//! direct push.
//!
//! # Content projection contract
//!
//! Outbound text is treated as **Markdown source** and projected through
//! [`prepare_markdown_for_platform`](crate::markdown::prepare_markdown_for_platform)
//! before length enforcement (projection changes length, so the order
//! matters): tables become linearized records, unsupported syntax degrades
//! per features. Text without Markdown syntax passes through
//! **byte-for-byte** (plain-text fast path), so system notices and
//! already-projected replies are stable — the chatbot reply path, which
//! projects in `flush_buffer` before segmenting, is unaffected when its
//! segments flow through this guard again at the adapter level (idempotent).
//! Hosts pushing literal plain text should avoid Markdown-significant
//! character pairs (e.g. `2*3*4`), which are interpreted as emphasis by
//! design.
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
use crate::markdown::prepare_markdown_for_platform;

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

/// Prepare `text` for sending on a platform described by `caps`:
/// **content projection first, then the length guard**.
///
/// - Text is projected to `caps.markdown_features` (tables linearized,
///   unsupported syntax degraded to readable plain text). Empty or
///   whitespace-only text, and text without Markdown syntax, pass through
///   byte-for-byte (plain-text fast path).
/// - If the projected text fits within `caps.max_message_length`, returns it
///   as-is in a single-element vector (idempotent pass-through).
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
/// the projected text is returned unchanged.
pub fn prepare_outbound(text: &str, caps: &PlatformCaps) -> OutboundResult {
    // Content projection (platform readability). Empty/whitespace-only text
    // keeps its exact bytes to preserve empty-segment semantics.
    let projected = if text.trim().is_empty() {
        text.to_string()
    } else {
        prepare_markdown_for_platform(text, &caps.markdown_features)
    };
    length_guard(&projected, caps)
}

/// Length enforcement on already-projected text (the original R16 guard).
fn length_guard(text: &str, caps: &PlatformCaps) -> OutboundResult {
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

    /// Non-edit platform that keeps code fences (QQ features): exercises the
    /// fence-aware split heuristic after content projection.
    fn non_edit_code_caps(max_len: usize) -> PlatformCaps {
        PlatformCaps {
            markdown_features: MarkdownFeatures::qq(),
            ..non_edit_caps(max_len)
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
        // QQ features keep fences through projection; the split heuristic
        // must still avoid cutting inside them.
        let caps = non_edit_code_caps(220);
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

    // -- Content projection (T3) ------------------------------------------

    #[test]
    fn outbound_projects_table_for_wechat_direct_call() {
        // Host direct push without the sanitizer: the guard projects anyway.
        let caps = non_edit_caps(2000);
        let md = "| 持仓 | 现价 |\n|---|---|\n| 贵州茅台 | 105.00 |\n";
        let result = prepare_outbound(md, &caps);
        assert_eq!(result.segments, vec!["贵州茅台: 105.00\n".to_string()]);
        assert!(!result.truncated);
    }

    #[test]
    fn outbound_projects_for_qq_direct_call() {
        let caps = edit_caps(2000);
        let md = "# 标题\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let result = prepare_outbound(md, &caps);
        assert_eq!(result.segments, vec!["标题\n\n1: 2\n".to_string()]);
    }

    #[test]
    fn outbound_projects_long_table_then_segments() {
        // Projection runs BEFORE segmentation: the linearized (longer) text
        // is what gets split, on record boundaries where possible.
        let caps = non_edit_caps(120);
        let mut md = String::from("| 持仓 | 现价 | 盈亏 | 仓位 |\n|---|---|---|---|\n");
        for i in 0..12 {
            md.push_str(&format!("| 股票{} | 105.00 | -4.59% | 28.0% |\n", i));
        }
        let result = prepare_outbound(&md, &caps);
        assert!(result.segments.len() >= 2);
        assert!(result.segments.len() <= MAX_OUTBOUND_SEGMENTS);
        assert!(result.segments.iter().all(|s| s.chars().count() <= 120));
        assert!(result.truncated);
        let joined = result.segments.concat();
        assert!(!joined.contains('|'), "pipe syntax must be projected away");
        assert!(joined.contains("股票0 — 现价: 105.00"));
    }

    #[test]
    fn outbound_projected_text_is_idempotent() {
        // The chatbot reply path projects in flush_buffer; the adapter guard
        // must not change those bytes again.
        let caps = non_edit_caps(2000);
        let md = "| 持仓 | 现价 | 盈亏 | 仓位 |\n|---|---|---|---|\n| 贵州茅台 | 105.00 | -4.59% | 28.0% |\n";
        let once = prepare_outbound(md, &caps);
        let twice = prepare_outbound(&once.segments.concat(), &caps);
        assert_eq!(once.segments, twice.segments);
        assert_eq!(once.truncated, twice.truncated);
    }

    #[test]
    fn outbound_wechat_strips_code_fences() {
        let caps = non_edit_caps(2000);
        let result = prepare_outbound("```\nlet x = 1;\n```\n", &caps);
        assert_eq!(result.segments, vec!["let x = 1;\n".to_string()]);
    }

    #[test]
    fn outbound_whitespace_only_passes_through() {
        let caps = non_edit_caps(2000);
        let result = prepare_outbound("  \n", &caps);
        assert_eq!(result.segments, vec!["  \n".to_string()]);
        assert!(!result.truncated);
    }

    #[test]
    fn outbound_system_notices_stay_byte_exact() {
        // Framework notices without Markdown syntax must not be touched.
        let caps = non_edit_caps(2000);
        for text in ["⏹️ 已发送停止信号", "ℹ️ 当前没有活动的会话", "❌ Error: boom"] {
            let result = prepare_outbound(text, &caps);
            assert_eq!(result.segments, vec![text.to_string()]);
        }
    }

    #[test]
    fn outbound_confirmation_prompt_keeps_payload() {
        // The confirmation prompt embeds pretty-printed JSON arguments.
        // Projection keeps every payload character (JSON indentation may be
        // normalized: paragraph continuation strips leading spaces).
        let caps = non_edit_caps(2000);
        let prompt = "⚠️ 需要确认工具调用\n\n工具: bash\n参数: {\n  \"command\": \"ls -la\"\n}\n\n回复 \"确认\" 或 \"取消\"\n(60秒内有效)";
        let result = prepare_outbound(prompt, &caps);
        let out = &result.segments[0];
        assert!(out.contains("⚠️ 需要确认工具调用"));
        assert!(out.contains("工具: bash"));
        assert!(out.contains("\"command\": \"ls -la\""));
        assert!(out.contains("回复 \"确认\" 或 \"取消\""));
        assert!(out.contains("(60秒内有效)"));
    }

    #[test]
    fn outbound_help_text_keeps_commands() {
        let caps = non_edit_caps(2000);
        let help = "🤖 Robit 帮助\n\n可用指令：\n- /clear - 清空当前对话上下文（仅内存中）\n- /switch <序号> - 切换到指定会话\n\n提示：直接发送消息与机器人对话即可。";
        let result = prepare_outbound(help, &caps);
        let out = &result.segments[0];
        assert!(out.contains("- /clear - 清空当前对话上下文（仅内存中）"));
        assert!(out.contains("/switch <序号>"));
        assert!(out.contains("提示：直接发送消息与机器人对话即可。"));
    }
}
