//! Context management — output truncation and history window management.

use async_openai::types::chat::ChatCompletionRequestMessage;
use robit_ai::config::ContextConfig;

// ============================================================================
// Truncation result
// ============================================================================

/// Result of context truncation, used for async compression.
#[derive(Debug)]
pub struct TruncationResult {
    /// Number of conversation rounds removed.
    pub rounds_removed: usize,
    /// Number of individual messages removed.
    pub messages_removed: usize,
    /// The removed messages (for generating summary).
    pub removed_messages: Vec<ChatCompletionRequestMessage>,
    /// Position where summary should be inserted.
    pub insert_position: usize,
    /// Whether compression is needed (token count exceeds threshold).
    pub needs_compression: bool,
}

// ============================================================================
// Tool output truncation (Layer 1)
// ============================================================================

/// Truncate tool output based on line count and byte limits.
pub fn truncate_output(content: &str, max_lines: usize, max_bytes: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    let total_bytes = content.len();

    // Check if truncation is needed
    let line_truncated = total_lines > max_lines;
    let byte_truncated = total_bytes > max_bytes;

    if !line_truncated && !byte_truncated {
        return content.to_string();
    }

    let mut output = String::new();
    let mut byte_count = 0;
    let mut displayed_lines = 0;

    for (i, line) in lines.iter().enumerate() {
        if i >= max_lines {
            break;
        }
        let line_with_newline = if i < total_lines - 1 {
            format!("{}\n", line)
        } else {
            line.to_string()
        };

        if byte_count + line_with_newline.len() > max_bytes {
            break;
        }

        output.push_str(&line_with_newline);
        byte_count += line_with_newline.len();
        displayed_lines += 1;
    }

    if line_truncated {
        output.push_str(&format!(
            "\n... (Output truncated, {} lines total, showing first {}. Use offset/limit to read more)",
            total_lines, displayed_lines
        ));
    } else if byte_truncated {
        output.push_str(&format!(
            "\n... (Output truncated, {} bytes total, showing first {} bytes)",
            total_bytes, byte_count
        ));
    }

    output
}

// ============================================================================
// Token estimation
// ============================================================================

/// Estimate token count for a string.
///
/// Uses a more nuanced heuristic based on character type:
/// - ASCII letters/digits: ~3.5 chars/token (BPE tokenizer average)
/// - CJK characters: ~1.5 chars/token (most CJK chars are 1-2 tokens each)
/// - Whitespace: minimal token cost (usually merged with adjacent tokens)
/// - Punctuation/symbols: ~1 token per char (often individual tokens)
/// - Code (braces, operators): ~1 token per char
///
/// This is still an estimate; apply `token_safety_margin` at the message level.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }

    let mut ascii_alnum = 0usize;
    let mut cjk = 0usize;
    let mut whitespace = 0usize;
    let mut other = 0usize; // punctuation, symbols, code characters

    for ch in text.chars() {
        if ch.is_whitespace() {
            whitespace += 1;
        } else if ch.is_ascii_alphanumeric() {
            ascii_alnum += 1;
        } else {
            let cp = ch as u32;
            // CJK Unified Ideographs + extensions + fullwidth forms
            // + Hiragana, Katakana, Hangul, CJK punctuation
            if (0x4E00..=0x9FFF).contains(&cp)
                || (0x3400..=0x4DBF).contains(&cp)
                || (0xF900..=0xFAFF).contains(&cp)
                || (0xFF00..=0xFFEF).contains(&cp)
                || (0x3000..=0x303F).contains(&cp)
                || (0x3040..=0x309F).contains(&cp)
                || (0x30A0..=0x30FF).contains(&cp)
                || (0xAC00..=0xD7AF).contains(&cp)
            {
                cjk += 1;
            } else {
                other += 1;
            }
        }
    }

    // BPE tokenizer averages:
    // - ASCII alphanumeric: ~3.5 chars per token
    // - CJK: ~1.5 chars per token (most are 1 token each, some pairs)
    // - Whitespace: negligible (merged with adjacent tokens)
    // - Other (punctuation/code): ~1 char per token
    let ascii_tokens = (ascii_alnum as f64 / 3.5).ceil() as usize;
    let cjk_tokens = (cjk as f64 / 1.5).ceil() as usize;
    let whitespace_tokens = (whitespace as f64 / 10.0).ceil() as usize;
    let other_tokens = other; // ~1:1

    ascii_tokens + cjk_tokens + whitespace_tokens + other_tokens
}

/// Estimate tokens for a list of messages.
pub fn estimate_messages_tokens(messages: &[ChatCompletionRequestMessage]) -> usize {
    let mut total = 0;
    for msg in messages {
        // Each message has ~4 tokens of overhead (role, delimiters)
        total += 4;
        total += estimate_message_content_tokens(msg);
    }
    total
}

/// Estimate tokens for messages, applying the configured safety margin.
pub fn estimate_messages_tokens_with_margin(
    messages: &[ChatCompletionRequestMessage],
    safety_margin: f32,
) -> usize {
    let raw = estimate_messages_tokens(messages);
    (raw as f32 * safety_margin).ceil() as usize
}

/// Estimate tokens for a single message's content.
fn estimate_message_content_tokens(msg: &ChatCompletionRequestMessage) -> usize {
    match serde_json::to_string(msg) {
        Ok(json) => estimate_tokens(&json),
        Err(_) => 0,
    }
}

// ============================================================================
// Context manager (Layer 2: history truncation)
// ============================================================================

/// Manages the context window, truncating history when approaching token limits.
pub struct ContextManager {
    /// Model's context window size in tokens.
    pub max_tokens: usize,
    /// Ratio of context window to reserve for LLM response (default 0.2 = 20%).
    pub reserve_ratio: f32,
    /// Fraction of max_tokens at which truncation triggers (default 0.7).
    pub truncation_ratio: f32,
    /// Minimum conversation rounds to keep after truncation (default 3).
    pub min_keep_rounds: usize,
    /// Safety multiplier for token estimates (default 1.3).
    pub token_safety_margin: f32,
    /// Max output lines for tool results.
    pub max_output_lines: usize,
    /// Max output bytes for tool results.
    pub max_output_bytes: usize,
    /// Token threshold for triggering compression.
    pub compression_token_threshold: usize,
    /// Whether compression is enabled.
    pub compression_enabled: bool,
}

impl ContextManager {
    pub fn new(context_window: Option<u64>, config: Option<&ContextConfig>) -> Self {
        let max_tokens = context_window.unwrap_or(65536) as usize;

        let (
            max_output_lines,
            max_output_bytes,
            reserve_ratio,
            truncation_ratio,
            min_keep_rounds,
            token_safety_margin,
            compression_token_threshold,
            compression_enabled,
        ) = match config {
            Some(c) => (
                c.max_output_lines.unwrap_or(500),
                c.max_output_bytes.unwrap_or(51200),
                c.reserve_ratio.unwrap_or(0.2),
                c.truncation_ratio.unwrap_or(0.7),
                c.min_keep_rounds.unwrap_or(3),
                c.token_safety_margin.unwrap_or(1.3),
                c.compression_token_threshold.unwrap_or(5000),
                c.compression_enabled.unwrap_or(true),
            ),
            None => (500, 51200, 0.2, 0.7, 3, 1.3, 5000, true),
        };

        Self {
            max_tokens,
            reserve_ratio,
            truncation_ratio,
            min_keep_rounds,
            token_safety_margin,
            max_output_lines,
            max_output_bytes,
            compression_token_threshold,
            compression_enabled,
        }
    }

    /// Maximum tokens at which truncation is triggered.
    /// Uses `truncation_ratio` (default 0.7) to trigger earlier than the
    /// absolute limit, leaving headroom for estimation errors and LLM response.
    pub fn truncation_threshold(&self) -> usize {
        (self.max_tokens as f32 * self.truncation_ratio) as usize
    }

    /// Maximum tokens available for input (total - reserved for response).
    /// Note: this is the absolute upper bound; truncation actually triggers
    /// earlier via `truncation_threshold()`.
    pub fn available_tokens(&self) -> usize {
        (self.max_tokens as f32 * (1.0 - self.reserve_ratio)) as usize
    }

    /// Truncate tool output using the configured limits.
    pub fn truncate_tool_output(&self, content: &str) -> String {
        truncate_output(content, self.max_output_lines, self.max_output_bytes)
    }

    /// Check if history needs truncation and perform it if necessary.
    /// Returns `TruncationResult` with removed messages for async compression.
    ///
    /// Strategy:
    /// 1. Uses `truncation_threshold()` (default 70% of max_tokens) as trigger point
    /// 2. Removes oldest non-system rounds first
    /// 3. Always keeps at least `min_keep_rounds` recent rounds
    /// 4. Applies `token_safety_margin` to estimates to avoid underestimation
    pub fn maybe_truncate(
        &self,
        messages: &mut Vec<ChatCompletionRequestMessage>,
    ) -> TruncationResult {
        let estimated = estimate_messages_tokens_with_margin(messages, self.token_safety_margin);
        let threshold = self.truncation_threshold();

        if estimated <= threshold {
            return TruncationResult {
                rounds_removed: 0,
                messages_removed: 0,
                removed_messages: Vec::new(),
                insert_position: 0,
                needs_compression: false,
            };
        }

        tracing::info!(
            "Context truncation needed: estimated {} tokens (with {:.1}x margin), threshold {} tokens, max {} tokens",
            estimated,
            self.token_safety_margin,
            threshold,
            self.max_tokens
        );

        // Find round boundaries: a round starts with a User message
        let mut round_starts: Vec<usize> = Vec::new();
        for (i, msg) in messages.iter().enumerate() {
            if is_user_message(msg) {
                round_starts.push(i);
            }
        }

        if round_starts.is_empty() {
            return TruncationResult {
                rounds_removed: 0,
                messages_removed: 0,
                removed_messages: Vec::new(),
                insert_position: 0,
                needs_compression: false,
            };
        }

        // Count how many rounds we must keep
        let total_rounds = round_starts.len();
        let must_keep = self.min_keep_rounds.min(total_rounds);

        let mut removed_messages: Vec<ChatCompletionRequestMessage> = Vec::new();
        let mut rounds_removed = 0;
        let mut messages_removed = 0;

        // Remove oldest rounds while:
        // - estimated tokens still exceed threshold
        // - we still have more rounds than must_keep
        while round_starts.len() > must_keep
            && estimate_messages_tokens_with_margin(messages, self.token_safety_margin) > threshold
        {
            let start_idx = round_starts[0];
            let end_idx = if round_starts.len() > 1 {
                round_starts[1]
            } else {
                messages.len()
            };

            // Collect removed messages for potential compression
            if self.compression_enabled {
                removed_messages.extend(messages[start_idx..end_idx].to_vec());
            }

            let count = end_idx - start_idx;
            messages.drain(start_idx..end_idx);

            // Update round_starts indices
            round_starts.remove(0);
            for idx in round_starts.iter_mut() {
                *idx = idx.saturating_sub(count);
            }

            rounds_removed += 1;
            messages_removed += count;
        }

        if rounds_removed == 0 {
            return TruncationResult {
                rounds_removed: 0,
                messages_removed: 0,
                removed_messages: Vec::new(),
                insert_position: 0,
                needs_compression: false,
            };
        }

        // Calculate removed tokens for threshold check
        let removed_tokens = estimate_messages_tokens(&removed_messages);
        let needs_compression =
            self.compression_enabled && removed_tokens >= self.compression_token_threshold;

        // Insert informative notice after system messages
        let system_msg_count = messages
            .iter()
            .take_while(|m| is_system_message(m))
            .count();

        let notice = if needs_compression {
            format!(
                "[Context compressed: {} earlier rounds ({} messages, ~{} tokens) have been summarized. {} most recent rounds preserved.]",
                rounds_removed,
                messages_removed,
                removed_tokens,
                round_starts.len()
            )
        } else {
            format!(
                "[Context truncated: {} earlier rounds ({} messages) removed to stay within token limit. {} most recent rounds preserved.]",
                rounds_removed,
                messages_removed,
                round_starts.len()
            )
        };

        let notice_msg = ChatCompletionRequestMessage::User(
            async_openai::types::chat::ChatCompletionRequestUserMessage {
                content: notice.into(),
                name: Some("system_notice".to_string()),
            }
            .into(),
        );

        messages.insert(system_msg_count, notice_msg);

        tracing::info!(
            "Context truncation: removed {} rounds ({} messages, ~{} tokens), kept {} rounds, needs_compression={}",
            rounds_removed,
            messages_removed,
            removed_tokens,
            round_starts.len(),
            needs_compression
        );

        TruncationResult {
            rounds_removed,
            messages_removed,
            removed_messages,
            insert_position: system_msg_count,
            needs_compression,
        }
    }
}

fn is_user_message(msg: &ChatCompletionRequestMessage) -> bool {
    matches!(msg, ChatCompletionRequestMessage::User(_))
}

fn is_system_message(msg: &ChatCompletionRequestMessage) -> bool {
    matches!(msg, ChatCompletionRequestMessage::System(_))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use async_openai::types::chat::{
        ChatCompletionRequestUserMessage,
    };

    fn make_user_message(content: &str) -> ChatCompletionRequestMessage {
        ChatCompletionRequestMessage::User(
            ChatCompletionRequestUserMessage {
                content: content.into(),
                name: None,
            }
            .into(),
        )
    }

    fn make_system_message(content: &str) -> ChatCompletionRequestMessage {
        ChatCompletionRequestMessage::System(
            async_openai::types::chat::ChatCompletionRequestSystemMessage {
                content: content.into(),
                name: None,
            }
            .into(),
        )
    }

    #[test]
    fn test_truncation_result_no_compression() {
        // Small messages - no truncation needed
        let mut messages = vec![
            make_system_message("You are a helpful assistant"),
            make_user_message("Hello"),
        ];

        let config = ContextConfig {
            max_output_lines: Some(500),
            max_output_bytes: Some(51200),
            reserve_ratio: Some(0.2),
            compression_token_threshold: Some(5000),
            compression_enabled: Some(true),
        };

        let manager = ContextManager::new(Some(65536), Some(&config));
        let result = manager.maybe_truncate(&mut messages);

        assert_eq!(result.rounds_removed, 0);
        assert!(!result.needs_compression);
    }

    #[test]
    fn test_truncation_result_with_compression() {
        // Create many large messages to exceed threshold
        let mut messages = vec![
            make_system_message("You are a helpful assistant"),
        ];

        // Add 20 rounds of large messages (each ~2000 chars = ~666 tokens)
        // Total: ~13,320 tokens, context window: 5000, available: 4000
        for i in 0..20 {
            let content = format!("User message {}: {}", i, "x".repeat(2000));
            messages.push(make_user_message(&content));
        }

        let config = ContextConfig {
            max_output_lines: Some(500),
            max_output_bytes: Some(51200),
            reserve_ratio: Some(0.2),
            compression_token_threshold: Some(1000), // Low threshold for testing
            compression_enabled: Some(true),
        };

        // Set small context window to force truncation
        let manager = ContextManager::new(Some(5000), Some(&config));
        let result = manager.maybe_truncate(&mut messages);

        assert!(result.rounds_removed > 0);
        assert!(result.needs_compression);
        assert!(!result.removed_messages.is_empty());
    }

    #[test]
    fn test_compression_disabled() {
        let mut messages = vec![
            make_system_message("You are a helpful assistant"),
        ];

        // Add 20 rounds of large messages
        for i in 0..20 {
            let content = format!("User message {}: {}", i, "x".repeat(2000));
            messages.push(make_user_message(&content));
        }

        let config = ContextConfig {
            max_output_lines: Some(500),
            max_output_bytes: Some(51200),
            reserve_ratio: Some(0.2),
            compression_token_threshold: Some(1000),
            compression_enabled: Some(false), // Disabled
        };

        let manager = ContextManager::new(Some(5000), Some(&config));
        let result = manager.maybe_truncate(&mut messages);

        assert!(result.rounds_removed > 0);
        assert!(!result.needs_compression); // Should be false even if threshold exceeded
    }

    #[test]
    fn test_estimate_tokens() {
        let text = "Hello world";
        let tokens = estimate_tokens(text);
        assert!(tokens > 0);
        assert!(tokens < 10); // ~11 chars / 4 = ~2-3 tokens

        let chinese = "你好世界";
        let tokens_cn = estimate_tokens(chinese);
        assert!(tokens_cn > 0);
        assert!(tokens_cn < 5); // ~4 chars / 2 = ~2 tokens
    }

    #[test]
    fn test_truncate_output() {
        let content = "line1\nline2\nline3\nline4\nline5";
        let truncated = truncate_output(content, 3, 100);
        assert!(truncated.contains("line1"));
        assert!(truncated.contains("line2"));
        assert!(truncated.contains("line3"));
        assert!(!truncated.contains("line4"));
        assert!(truncated.contains("Output truncated"));
    }
}
