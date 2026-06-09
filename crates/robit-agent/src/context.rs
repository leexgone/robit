//! Context management — output truncation and history window management.

use async_openai::types::chat::ChatCompletionRequestMessage;
use robit_ai::config::ContextConfig;

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
            "\n... (输出已截断，共 {} 行，显示前 {} 行。请使用 offset/limit 参数分段读取)",
            total_lines, displayed_lines
        ));
    } else if byte_truncated {
        output.push_str(&format!(
            "\n... (输出已截断，共 {} bytes，显示前 {} bytes)",
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
    /// Max output lines for tool results.
    pub max_output_lines: usize,
    /// Max output bytes for tool results.
    pub max_output_bytes: usize,
}

impl ContextManager {
    pub fn new(context_window: Option<u64>, config: Option<&ContextConfig>) -> Self {
        let max_tokens = context_window.unwrap_or(65536) as usize;

        let (max_output_lines, max_output_bytes, reserve_ratio) = match config {
            Some(c) => (
                c.max_output_lines.unwrap_or(500),
                c.max_output_bytes.unwrap_or(51200),
                c.reserve_ratio.unwrap_or(0.2),
            ),
            None => (500, 51200, 0.2),
        };

        Self {
            max_tokens,
            reserve_ratio,
            max_output_lines,
            max_output_bytes,
        }
    }

    /// Maximum tokens available for input (total - reserved for response).
    pub fn available_tokens(&self) -> usize {
        (self.max_tokens as f32 * (1.0 - self.reserve_ratio)) as usize
    }

    /// Truncate tool output using the configured limits.
    pub fn truncate_tool_output(&self, content: &str) -> String {
        truncate_output(content, self.max_output_lines, self.max_output_bytes)
    }

    /// Check if history needs truncation and perform it if necessary.
    /// Returns the number of rounds removed.
    ///
    /// Strategy: remove oldest non-system messages by "rounds"
    /// (user + assistant + tool_calls + tool_results grouped together).
    pub fn maybe_truncate(&self, messages: &mut Vec<ChatCompletionRequestMessage>) -> usize {
        let estimated = estimate_messages_tokens(messages);
        let available = self.available_tokens();

        if estimated <= available {
            return 0;
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
            // Insert a summary notice as the second message (after system message)
            let notice = format!(
                "[已省略 {} 轮对话，共 {} 条消息]",
                rounds_removed, messages_removed
            );

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
                "Removed {} rounds ({} messages), inserted summary notice",
                rounds_removed,
                messages_removed
            );
        }

        rounds_removed
    }
}

fn is_user_message(msg: &ChatCompletionRequestMessage) -> bool {
    matches!(msg, ChatCompletionRequestMessage::User(_))
}

fn is_system_message(msg: &ChatCompletionRequestMessage) -> bool {
    matches!(msg, ChatCompletionRequestMessage::System(_))
}
