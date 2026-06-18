//! Per-session Frontend implementation for Bot platforms.
//!
//! [`ChatbotFrontend`] implements [`robit_agent::frontend::Frontend`] for a
//! single chat (group or private). It buffers streaming `TextDelta` events and
//! flushes them in natural-boundary segments (so Markdown and code blocks
//! aren't cut mid-construct), sends a rate-limited progress hint when tools
//! run, and delegates tool confirmation to the shared [`Confirmer`].
//!
//! On platforms with edit support, the first message is sent then edited in
//! place to create a "growing message" effect; otherwise segments are sent as
//! separate messages.

use std::sync::Arc;

use async_trait::async_trait;
use robit_agent::error::Result;
use robit_agent::event::AgentEvent;
use robit_agent::frontend::Frontend;
use robit_agent::tool::ToolCallInfo;
use tokio::sync::Mutex;

use crate::adapter::{PlatformCaps, SendResult};
use crate::confirmer::Confirmer;
use crate::markdown::prepare_markdown_for_platform;

/// Buffer threshold (in characters) at which to flush a streaming segment.
const FLUSH_THRESHOLD: usize = 200;

/// Abstracted message sending capability (platform-agnostic).
///
/// `ChatbotFrontend` talks to the platform through this trait rather than
/// `PlatformAdapter` directly, so the manager can supply a bridge that wraps
/// the concrete adapter.
#[async_trait]
pub trait PlatformSender: Send + Sync {
    /// Send a text message to a chat; returns the platform message ID.
    async fn send(&self, chat_id: &str, text: &str) -> Result<SendResult>;
    /// Edit a previously-sent message in place.
    async fn edit(&self, chat_id: &str, msg_id: &str, text: &str) -> Result<()>;
    /// Platform capabilities (drives streaming strategy).
    fn capabilities(&self) -> PlatformCaps;
}

/// Per-session Frontend trait implementation for Bot platforms.
///
/// Each chat (group or private) gets its own `ChatbotFrontend` instance.
/// `TextDelta` events are buffered and flushed in smart segments.
pub struct ChatbotFrontend {
    /// The chat this frontend belongs to (`group:{id}` or `private:{id}`).
    pub chat_id: String,
    /// Platform message sender (shared across all frontends).
    pub platform_sender: Arc<dyn PlatformSender>,
    /// Tool confirmation coordinator (shared).
    pub confirmer: Arc<Confirmer>,
    /// Streaming text buffer.
    pub buffer: Mutex<String>,
    /// ID of the last message sent (for edit-based streaming updates).
    pub last_msg_id: Mutex<Option<String>>,
    /// Whether a progress hint has already been sent this turn (rate limit).
    pub progress_hint_sent: Mutex<bool>,
    /// Auto-approve all tool calls.
    pub auto_approve: bool,
}

impl ChatbotFrontend {
    /// Create a new `ChatbotFrontend` for `chat_id`.
    pub fn new(
        chat_id: String,
        platform_sender: Arc<dyn PlatformSender>,
        confirmer: Arc<Confirmer>,
        auto_approve: bool,
    ) -> Self {
        Self {
            chat_id,
            platform_sender,
            confirmer,
            buffer: Mutex::new(String::new()),
            last_msg_id: Mutex::new(None),
            progress_hint_sent: Mutex::new(false),
            auto_approve,
        }
    }

    /// Append a delta to the buffer and flush a segment if a natural boundary
    /// is reached.
    async fn append_delta(&self, delta: &str) {
        let mut buffer = self.buffer.lock().await;
        buffer.push_str(delta);
        if buffer.chars().count() >= FLUSH_THRESHOLD {
            if let Some((segment, remainder)) = split_at_natural_boundary(&buffer, FLUSH_THRESHOLD)
            {
                *buffer = remainder;
                drop(buffer); // release lock before sending
                self.send_segment(&segment).await;
            }
        }
    }

    /// Flush all remaining buffered content.
    async fn flush_buffer(&self) {
        let mut buffer = self.buffer.lock().await;
        if buffer.is_empty() {
            return;
        }
        let text = std::mem::take(&mut *buffer);
        drop(buffer);
        self.send_segment(&text).await;
    }

    /// Send a text segment, applying Markdown sanitization. Uses edit-based
    /// streaming (accumulated text) when the platform supports it and a
    /// previous message exists; otherwise sends a new message.
    async fn send_segment(&self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        let caps = self.platform_sender.capabilities();
        let prepared = if caps.supports_markdown {
            prepare_markdown_for_platform(text, &caps.markdown_features)
        } else {
            text.to_string()
        };
        let prepared = truncate_to_max(&prepared, caps.max_message_length);

        let mut last_msg_id = self.last_msg_id.lock().await;
        if caps.supports_edit && last_msg_id.is_some() {
            // Edit in place: accumulate the full text so the message "grows".
            // We store accumulated text by editing to the segment itself; for
            // simplicity in MVP we edit to the current segment (no accumulated
            // state kept). A future improvement can accumulate across segments.
            let msg_id = last_msg_id.clone().unwrap();
            if self
                .platform_sender
                .edit(&self.chat_id, &msg_id, &prepared)
                .await
                .is_err()
            {
                // Edit failed — fall back to a fresh send.
                if let Ok(res) = self.platform_sender.send(&self.chat_id, &prepared).await {
                    *last_msg_id = Some(res.msg_id);
                }
            }
        } else {
            if let Ok(res) = self.platform_sender.send(&self.chat_id, &prepared).await {
                *last_msg_id = Some(res.msg_id);
            }
        }
    }

    /// Send a brief progress hint. Only sends once per turn to avoid spam.
    async fn send_progress_hint(&self, tool_name: &str) {
        let mut sent = self.progress_hint_sent.lock().await;
        if *sent {
            return;
        }
        *sent = true;
        drop(sent);

        let hint = match tool_name {
            "bash" => "🔧 正在执行命令...".to_string(),
            "read" => "📖 正在读取文件...".to_string(),
            "write" => "✏️ 正在写入文件...".to_string(),
            "edit" => "✏️ 正在编辑文件...".to_string(),
            "grep" => "🔍 正在搜索...".to_string(),
            "find" => "🔍 正在查找...".to_string(),
            _ => "🔧 正在处理...".to_string(),
        };
        if let Ok(res) = self.platform_sender.send(&self.chat_id, &hint).await {
            *self.last_msg_id.lock().await = Some(res.msg_id);
        }
    }
}

#[async_trait]
impl Frontend for ChatbotFrontend {
    async fn on_event(&self, event: AgentEvent) -> Result<()> {
        match event {
            AgentEvent::TextDelta(delta) => {
                self.append_delta(&delta).await;
            }
            AgentEvent::ToolCallRequested { name, .. } => {
                // Flush any buffered text before showing tool progress.
                self.flush_buffer().await;
                // In auto-approve mode, send a progress hint so the user knows
                // the bot is working. In manual mode, the Confirmer already
                // sends a confirm prompt — no extra hint needed.
                if self.auto_approve {
                    self.send_progress_hint(&name).await;
                }
            }
            AgentEvent::ToolCallResult { .. } => {
                // Silent: tool outputs are internal; the user only sees the
                // final text reply. Any progress hint is replaced by the reply
                // on TurnComplete.
            }
            AgentEvent::TurnComplete => {
                self.flush_buffer().await;
                // Reset per-turn state.
                *self.progress_hint_sent.lock().await = false;
                *self.last_msg_id.lock().await = None;
            }
            AgentEvent::Error(e) => {
                self.flush_buffer().await;
                let msg = format!("❌ Error: {}", e);
                let _ = self.platform_sender.send(&self.chat_id, &msg).await;
            }
            AgentEvent::SkillTriggered { .. } => {
                // Silent: skill trigger is internal; the skill's own output
                // arrives as TextDelta events.
            }
        }
        Ok(())
    }

    async fn request_tool_confirmation(&self, info: &ToolCallInfo) -> Result<bool> {
        self.confirmer
            .request(&self.chat_id, info, self.auto_approve)
            .await
    }
}

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

/// Find the best natural break point at or before `threshold` characters and
/// split the buffer into `(segment, remainder)`.
///
/// Priority (searching backward from `threshold`):
/// 1. Paragraph break (`\n\n`)
/// 2. Code block fence end (` ``` `)
/// 3. Line break (`\n`)
/// 4. Sentence end (`.` `。` `!` `！` `?` `？` followed by space or end)
/// 5. Space (word boundary)
///
/// Returns `None` if no suitable break point is found (caller keeps buffering).
fn split_at_natural_boundary(buffer: &str, threshold: usize) -> Option<(String, String)> {
    let chars: Vec<char> = buffer.chars().collect();
    if chars.len() < threshold {
        return None;
    }
    // Search window: from the start up to `threshold` (plus a little slack to
    // avoid cutting a paragraph break that straddles the boundary).
    let window_end = threshold.min(chars.len());

    // 1. Paragraph break (\n\n) — split after the second newline.
    let mut best: Option<usize> = None;
    for i in 1..window_end {
        if chars[i - 1] == '\n' && chars[i] == '\n' {
            best = Some(i + 1); // include the blank line in the segment
        }
    }
    if let Some(pos) = best {
        return Some(split_chars(&chars, pos));
    }

    // 2. Code block fence end (```).
    let s: String = chars[..window_end].iter().collect();
    if let Some(idx) = s.rfind("```") {
        let pos = idx + 3;
        return Some(split_chars(&chars, pos));
    }

    // 3. Line break.
    for i in (0..window_end).rev() {
        if chars[i] == '\n' {
            return Some(split_chars(&chars, i + 1));
        }
    }

    // 4. Sentence end.
    for i in (0..window_end).rev() {
        let c = chars[i];
        if matches!(c, '.' | '。' | '!' | '！' | '?' | '？') {
            return Some(split_chars(&chars, i + 1));
        }
    }

    // 5. Space (word boundary).
    for i in (0..window_end).rev() {
        if chars[i] == ' ' {
            return Some(split_chars(&chars, i + 1));
        }
    }

    None
}

/// Split a char slice at `pos` into two owned strings.
fn split_chars(chars: &[char], pos: usize) -> (String, String) {
    let pos = pos.min(chars.len());
    let segment: String = chars[..pos].iter().collect();
    let remainder: String = chars[pos..].iter().collect();
    (segment, remainder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::MarkdownFeatures;
    use std::sync::Mutex as StdMutex;

    struct MockSender {
        sent: StdMutex<Vec<(String, String)>>,
        edits: StdMutex<Vec<(String, String, String)>>,
        caps: PlatformCaps,
    }

    impl MockSender {
        fn new_with_edit(edit: bool) -> Arc<Self> {
            Arc::new(Self {
                sent: StdMutex::new(Vec::new()),
                edits: StdMutex::new(Vec::new()),
                caps: PlatformCaps {
                    supports_edit: edit,
                    returns_msg_id: true,
                    supports_markdown: true,
                    markdown_features: MarkdownFeatures::qq(),
                    max_message_length: 2000,
                },
            })
        }
        fn sent_texts(&self) -> Vec<String> {
            self.sent.lock().unwrap().iter().map(|(_, t)| t.clone()).collect()
        }
    }

    #[async_trait]
    impl PlatformSender for MockSender {
        async fn send(&self, chat_id: &str, text: &str) -> Result<SendResult> {
            self.sent
                .lock()
                .unwrap()
                .push((chat_id.to_string(), text.to_string()));
            Ok(SendResult {
                msg_id: format!("msg-{}", self.sent.lock().unwrap().len()),
            })
        }
        async fn edit(&self, chat_id: &str, msg_id: &str, text: &str) -> Result<()> {
            self.edits.lock().unwrap().push((
                chat_id.to_string(),
                msg_id.to_string(),
                text.to_string(),
            ));
            Ok(())
        }
        fn capabilities(&self) -> PlatformCaps {
            self.caps.clone()
        }
    }

    fn make_frontend(sender: Arc<dyn PlatformSender>, auto_approve: bool) -> ChatbotFrontend {
        let confirmer = Arc::new(Confirmer::new(sender.clone(), std::time::Duration::from_secs(60)));
        ChatbotFrontend::new("group:1".to_string(), sender, confirmer, auto_approve)
    }

    #[tokio::test]
    async fn textdelta_accumulates_until_threshold() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), false);
        // Small deltas below threshold should not send.
        fe.on_event(AgentEvent::TextDelta("短文本".to_string())).await.unwrap();
        assert!(sender.sent_texts().is_empty());
    }

    #[tokio::test]
    async fn turn_complete_flushes_remaining_buffer() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), false);
        fe.on_event(AgentEvent::TextDelta("一段未被刷新的文本".to_string())).await.unwrap();
        assert!(sender.sent_texts().is_empty());
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();
        assert_eq!(sender.sent_texts(), vec!["一段未被刷新的文本\n".to_string()]);
    }

    #[tokio::test]
    async fn large_delta_flushes_at_paragraph_boundary() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), false);
        // Build text with a paragraph break past the threshold.
        let part1 = "x".repeat(180);
        let part2 = "\n\n".to_string() + &"y".repeat(100);
        let text = format!("{}{}", part1, part2);
        fe.on_event(AgentEvent::TextDelta(text)).await.unwrap();
        // A segment should have been flushed at the paragraph break.
        let sent = sender.sent_texts();
        assert!(!sent.is_empty());
        assert!(sent[0].contains(&part1));
    }

    #[tokio::test]
    async fn progress_hint_rate_limited_per_turn() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), true); // auto_approve
        // Two tool calls in one turn — only one hint should be sent.
        fe.on_event(AgentEvent::ToolCallRequested {
            tool_call_id: "tc1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
        }).await.unwrap();
        fe.on_event(AgentEvent::ToolCallRequested {
            tool_call_id: "tc2".into(),
            name: "read".into(),
            arguments: "{}".into(),
        }).await.unwrap();
        let hints: Vec<_> = sender
            .sent_texts()
            .into_iter()
            .filter(|t| t.contains("正在"))
            .collect();
        assert_eq!(hints.len(), 1);
    }

    #[tokio::test]
    async fn progress_hint_resets_on_turn_complete() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), true);
        fe.on_event(AgentEvent::ToolCallRequested {
            tool_call_id: "tc1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
        }).await.unwrap();
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();
        // After TurnComplete, a new tool call should send another hint.
        fe.on_event(AgentEvent::ToolCallRequested {
            tool_call_id: "tc2".into(),
            name: "bash".into(),
            arguments: "{}".into(),
        }).await.unwrap();
        let hints: Vec<_> = sender
            .sent_texts()
            .into_iter()
            .filter(|t| t.contains("正在"))
            .collect();
        assert_eq!(hints.len(), 2);
    }

    #[tokio::test]
    async fn no_hint_in_manual_mode() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), false); // manual confirm
        fe.on_event(AgentEvent::ToolCallRequested {
            tool_call_id: "tc1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
        }).await.unwrap();
        // No progress hint in manual mode (Confirmer sends the prompt instead).
        assert!(!sender.sent_texts().iter().any(|t| t.contains("正在")));
    }

    #[tokio::test]
    async fn error_sends_error_message() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), false);
        fe.on_event(AgentEvent::Error(robit_agent::AgentError::ToolError("boom".into())))
            .await
            .unwrap();
        assert!(sender.sent_texts().iter().any(|t| t.contains("Error") && t.contains("boom")));
    }

    #[test]
    fn split_at_sentence_end() {
        let (seg, rem) = split_at_natural_boundary(
            "Hello world. Next sentence here.",
            15,
        ).unwrap();
        assert!(seg.contains("Hello world."));
        assert!(rem.starts_with(" Next"));
    }

    #[test]
    fn split_returns_none_below_threshold() {
        assert!(split_at_natural_boundary("short", 200).is_none());
    }
}
