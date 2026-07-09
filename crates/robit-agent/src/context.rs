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
/// Uses a simple heuristic: ~4 chars per English token, ~2 chars per Chinese token.
/// For mixed content, we use ~3 chars per token as a rough estimate.
pub fn estimate_tokens(text: &str) -> usize {
    let char_count = text.chars().count();
    // Count CJK characters (rough heuristic)
    let cjk_count = text
        .chars()
        .filter(|c| {
            let cp = *c as u32;
            // CJK Unified Ideographs + extensions + fullwidth forms
            (0x4E00..=0x9FFF).contains(&cp)
                || (0x3400..=0x4DBF).contains(&cp)
                || (0xF900..=0xFAFF).contains(&cp)
                || (0xFF00..=0xFFEF).contains(&cp)
        })
        .count();

    let non_cjk = char_count.saturating_sub(cjk_count);
    let cjk_tokens = cjk_count / 2;
    let non_cjk_tokens = non_cjk / 4;

    cjk_tokens + non_cjk_tokens
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

/// Estimate tokens for a single message's content.
fn estimate_message_content_tokens(msg: &ChatCompletionRequestMessage) -> usize {
    // We need to extract the text content from the message.
    // Since ChatCompletionRequestMessage is an enum, we match on variants.
    // For simplicity, serialize to JSON and estimate from the string.
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
    /// Strategy: remove oldest non-system messages by "rounds"
    /// (user + assistant + tool_calls + tool_results grouped together).
    pub fn maybe_truncate(
        &self,
        messages: &mut Vec<ChatCompletionRequestMessage>,
    ) -> TruncationResult {
        let estimated = estimate_messages_tokens(messages);
        let available = self.available_tokens();

        if estimated <= available {
            return TruncationResult {
                rounds_removed: 0,
                messages_removed: 0,
                removed_messages: Vec::new(),
                insert_position: 0,
                needs_compression: false,
            };
        }

        tracing::info!(
            "Context truncation needed: estimated {} tokens, available {} tokens",
            estimated,
            available
        );

        // Find rounds to remove (skip system messages at the start)
        let mut rounds_removed = 0;
        let mut messages_removed = 0;

        // Group messages into rounds: a round starts with a User message
        // and includes all subsequent messages until the next User message
        let mut round_starts: Vec<usize> = Vec::new();
        for (i, msg) in messages.iter().enumerate() {
            if is_user_message(msg) && i > 0 {
                // Skip the first user message if there's a system message before it
                round_starts.push(i);
            } else if is_user_message(msg) && i == 0 {
                round_starts.push(i);
            }
        }

        // Collect removed messages for potential compression
        let mut removed_messages: Vec<ChatCompletionRequestMessage> = Vec::new();

        // Remove rounds from the oldest first
        // Keep at least the system message
        while !round_starts.is_empty() && estimate_messages_tokens(messages) > available {
            // Determine the range of the oldest round to remove
            let start_idx = round_starts[0];
            let end_idx = if round_starts.len() > 1 {
                round_starts[1]
            } else {
                messages.len()
            };

            // Don't remove if it would leave us with only system messages
            // and no user messages
            let remaining_user_msgs = messages[end_idx..]
                .iter()
                .filter(|m| is_user_message(m))
                .count();
            if remaining_user_msgs == 0 {
                break;
            }

            // Collect messages before removing
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

        if rounds_removed > 0 {
            // Calculate removed tokens for threshold check
            let removed_tokens = estimate_messages_tokens(&removed_messages);
            let needs_compression =
                self.compression_enabled && removed_tokens >= self.compression_token_threshold;

            // Insert placeholder or wait for summary
            let notice = if needs_compression {
                "[Generating conversation summary...]"
            } else {
                &format!("[Omitted {} rounds, {} messages]", rounds_removed, messages_removed)
            };

            let system_msg_count = messages
                .iter()
                .take_while(|m| is_system_message(m))
                .count();

            let notice_msg = ChatCompletionRequestMessage::User(
                async_openai::types::chat::ChatCompletionRequestUserMessage {
                    content: notice.into(),
                    name: Some("system_notice".to_string()),
                }
                .into(),
            );

            messages.insert(system_msg_count, notice_msg);

            tracing::info!(
                "Removed {} rounds ({} messages), needs_compression={}, removed_tokens={}",
                rounds_removed,
                messages_removed,
                needs_compression,
                removed_tokens
            );

            return TruncationResult {
                rounds_removed,
                messages_removed,
                removed_messages,
                insert_position: system_msg_count,
                needs_compression,
            };
        }

        TruncationResult {
            rounds_removed: 0,
            messages_removed: 0,
            removed_messages: Vec::new(),
            insert_position: 0,
            needs_compression: false,
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
