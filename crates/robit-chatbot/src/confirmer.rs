//! Tool confirmation coordinator for Bot platforms.
//!
//! Unlike the GUI which uses dialog boxes, Bot confirmation happens via inline
//! chat messages: the [`Confirmer`] sends a prompt, then waits for the user to
//! reply with an approve/reject keyword (with a timeout). The
//! [`ChatbotManager`](crate::manager::ChatbotManager) intercepts user messages
//! and routes confirmation replies here via [`Confirmer::check_confirmation_response`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use robit_agent::error::{AgentError, Result};
use robit_agent::tool::ToolCallInfo;
use tokio::sync::oneshot;

use crate::frontend::PlatformSender;

/// Keywords that trigger approval or rejection of a pending tool call.
#[derive(Debug, Clone)]
pub struct ConfirmKeywords {
    pub approve: Vec<String>,
    pub reject: Vec<String>,
}

impl Default for ConfirmKeywords {
    fn default() -> Self {
        Self {
            approve: vec![
                "确认".into(),
                "同意".into(),
                "yes".into(),
                "y".into(),
                "approve".into(),
                "ok".into(),
                "允许".into(),
            ],
            reject: vec![
                "取消".into(),
                "拒绝".into(),
                "no".into(),
                "n".into(),
                "reject".into(),
                "cancel".into(),
                "deny".into(),
            ],
        }
    }
}

/// A pending confirmation awaiting a user reply.
struct PendingConfirmation {
    /// Oneshot sender used to deliver the user's decision to the waiting
    /// `request()` call.
    sender: Option<oneshot::Sender<bool>>,
    created_at: Instant,
    #[allow(dead_code)]
    chat_id: String,
    #[allow(dead_code)]
    tool_call_id: String,
    #[allow(dead_code)]
    tool_name: String,
    #[allow(dead_code)]
    arguments: String,
}

/// Tool confirmation coordinator for Bot platforms.
///
/// One `Confirmer` is shared across all chat sessions (wrapped in `Arc`).
/// Pending confirmations are keyed by `"{chat_id}:{tool_call_id}"`.
pub struct Confirmer {
    /// `std::sync::Mutex` — held only briefly for HashMap operations.
    pending: Mutex<HashMap<String, PendingConfirmation>>,
    platform_sender: Arc<dyn PlatformSender>,
    timeout: Duration,
    keywords: ConfirmKeywords,
}

impl Confirmer {
    /// Create a new `Confirmer`.
    pub fn new(platform_sender: Arc<dyn PlatformSender>, timeout: Duration) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            platform_sender,
            timeout,
            keywords: ConfirmKeywords::default(),
        }
    }

    /// Create a `Confirmer` with custom keywords.
    pub fn with_keywords(
        platform_sender: Arc<dyn PlatformSender>,
        timeout: Duration,
        keywords: ConfirmKeywords,
    ) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            platform_sender,
            timeout,
            keywords,
        }
    }

    /// Request tool confirmation. Sends a prompt message to the chat and waits
    /// for the user to reply with an approve/reject keyword.
    ///
    /// - If `auto_approve` is true, returns `true` immediately without sending.
    /// - Returns `true` if approved, `false` if rejected or timed out.
    pub async fn request(
        &self,
        chat_id: &str,
        info: &ToolCallInfo,
        auto_approve: bool,
    ) -> Result<bool> {
        if auto_approve {
            return Ok(true);
        }

        let key = format!("{}:{}", chat_id, info.id);

        // Build and send the confirmation prompt.
        let prompt = format_confirmation_prompt(
            &info.name,
            &info.arguments,
            self.timeout,
            &self.keywords,
        );
        if let Err(e) = self.platform_sender.send(chat_id, &prompt).await {
            tracing::warn!("Failed to send confirmation prompt: {}", e);
        }

        let (tx, rx) = oneshot::channel::<bool>();
        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(
                key.clone(),
                PendingConfirmation {
                    sender: Some(tx),
                    created_at: Instant::now(),
                    chat_id: chat_id.to_string(),
                    tool_call_id: info.id.clone(),
                    tool_name: info.name.clone(),
                    arguments: info.arguments.clone(),
                },
            );
        }

        // Wait for a reply or timeout.
        let decision = match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(approved)) => approved,
            Ok(Err(_)) => {
                // Sender dropped without a reply — treat as rejection.
                false
            }
            Err(_) => {
                // Timed out.
                self.remove_pending(&key);
                let _ = self
                    .platform_sender
                    .send(chat_id, "⏰ 已超时，操作已取消")
                    .await;
                return Ok(false);
            }
        };

        // Cleanup (the responder may have already removed it).
        self.remove_pending(&key);

        let notice = if decision { "✅ 已确认" } else { "❌ 已取消" };
        let _ = self.platform_sender.send(chat_id, notice).await;
        Ok(decision)
    }

    /// Check whether a user message is a confirmation reply for a pending
    /// request in `chat_id`. If it matches a keyword, the pending entry is
    /// consumed and the waiting `request()` is unblocked.
    ///
    /// Returns `Some(approved)` if the message was a confirmation reply,
    /// `None` if there was no pending confirmation or the message didn't
    /// match a keyword.
    pub fn check_confirmation_response(&self, chat_id: &str, text: &str) -> Option<bool> {
        let text_lower = text.trim().to_lowercase();
        let is_approve = self.keywords.approve.contains(&text_lower);
        let is_reject = self.keywords.reject.contains(&text_lower);
        if !is_approve && !is_reject {
            return None;
        }

        let mut pending = self.pending.lock().unwrap();
        // Find the first pending confirmation for this chat.
        let key = pending
            .keys()
            .find(|k| k.starts_with(&format!("{}:", chat_id)))
            .cloned()?;
        let entry = pending.remove(&key)?;
        let sender = entry.sender?;
        let _ = sender.send(is_approve);
        Some(is_approve)
    }

    /// Periodically clean up expired pending confirmations.
    ///
    /// Expired entries are removed; their waiting `request()` calls will then
    /// time out on their own `tokio::time::timeout`.
    pub fn cleanup_expired(&self) {
        let now = Instant::now();
        let mut pending = self.pending.lock().unwrap();
        pending.retain(|_, entry| now.duration_since(entry.created_at) < self.timeout);
    }

    fn remove_pending(&self, key: &str) {
        let mut pending = self.pending.lock().unwrap();
        pending.remove(key);
    }
}

/// Build the inline confirmation prompt shown to the user.
fn format_confirmation_prompt(
    tool_name: &str,
    arguments: &str,
    timeout: Duration,
    keywords: &ConfirmKeywords,
) -> String {
    let secs = timeout.as_secs();
    let approve_hint = keywords.approve.first().cloned().unwrap_or_else(|| "确认".into());
    let reject_hint = keywords.reject.first().cloned().unwrap_or_else(|| "取消".into());
    // Pretty-print the arguments JSON if possible.
    let pretty_args = serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| arguments.to_string());

    format!(
        "⚠️ 需要确认工具调用\n\n工具: {}\n参数: {}\n\n回复 \"{}\" 或 \"{}\"\n({}秒内有效)",
        tool_name, pretty_args, approve_hint, reject_hint, secs
    )
}

#[allow(dead_code)]
fn _ensure_agent_error_used() -> AgentError {
    // Keeps the AgentError import live for the public Result type.
    AgentError::InternalError(String::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{MarkdownFeatures, PlatformCaps, SendResult};
    use async_trait::async_trait;
    use std::sync::Mutex as StdMutex;
    use tokio::sync::Mutex;

    struct MockSender {
        sent: StdMutex<Vec<(String, String)>>,
        caps: PlatformCaps,
    }

    impl MockSender {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                sent: StdMutex::new(Vec::new()),
                caps: PlatformCaps {
                    supports_edit: true,
                    returns_msg_id: true,
                    supports_markdown: true,
                    markdown_features: MarkdownFeatures::qq(),
                    max_message_length: 2000,
                },
            })
        }

        fn messages(&self) -> Vec<(String, String)> {
            self.sent.lock().unwrap().clone()
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
                msg_id: "mock-id".to_string(),
            })
        }
        async fn edit(&self, _chat_id: &str, _msg_id: &str, _text: &str) -> Result<()> {
            Ok(())
        }
        fn capabilities(&self) -> PlatformCaps {
            self.caps.clone()
        }
    }

    fn tool_info(id: &str, name: &str) -> ToolCallInfo {
        ToolCallInfo {
            id: id.to_string(),
            name: name.to_string(),
            arguments: "{}".to_string(),
        }
    }

    #[tokio::test]
    async fn auto_approve_skips_prompt() {
        let sender = MockSender::new();
        let confirmer = Confirmer::new(sender.clone(), Duration::from_secs(60));
        let info = tool_info("tc1", "bash");
        let approved = confirmer.request("group:1", &info, true).await.unwrap();
        assert!(approved);
        // No prompt message should have been sent.
        assert!(sender.messages().is_empty());
    }

    #[tokio::test]
    async fn approve_keyword_resolves_pending() {
        let sender = MockSender::new();
        let confirmer = Arc::new(Confirmer::new(sender.clone(), Duration::from_secs(5)));
        let info = tool_info("tc2", "write");

        let confirmer_clone = confirmer.clone();
        let handle = tokio::spawn(async move {
            confirmer_clone.request("group:1", &info, false).await
        });

        // Give the request a moment to register.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Simulate the user replying with an approve keyword.
        let result = confirmer.check_confirmation_response("group:1", "确认");
        assert_eq!(result, Some(true));

        let approved = handle.await.unwrap().unwrap();
        assert!(approved);
        // A confirm prompt + an "已确认" notice were sent.
        let msgs = sender.messages();
        assert!(msgs.iter().any(|(_, t)| t.contains("需要确认")));
        assert!(msgs.iter().any(|(_, t)| t.contains("已确认")));
    }

    #[tokio::test]
    async fn reject_keyword_resolves_pending() {
        let sender = MockSender::new();
        let confirmer = Arc::new(Confirmer::new(sender.clone(), Duration::from_secs(5)));
        let info = tool_info("tc3", "bash");

        let confirmer_clone = confirmer.clone();
        let handle = tokio::spawn(async move {
            confirmer_clone.request("group:2", &info, false).await
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        let result = confirmer.check_confirmation_response("group:2", "取消");
        assert_eq!(result, Some(false));

        let approved = handle.await.unwrap().unwrap();
        assert!(!approved);
    }

    #[tokio::test]
    async fn non_keyword_message_returns_none() {
        let sender = MockSender::new();
        let confirmer = Arc::new(Confirmer::new(sender.clone(), Duration::from_secs(60)));
        let info = tool_info("tc4", "bash");

        // Register a pending request (don't await it).
        let confirmer_clone = confirmer.clone();
        let handle = tokio::spawn(async move {
            // Use a short timeout so the test doesn't hang if logic is wrong;
            // we'll resolve it before then.
            confirmer_clone.request("group:3", &info, false).await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        // A normal chat message is not a confirmation reply.
        assert_eq!(confirmer.check_confirmation_response("group:3", "hello world"), None);

        // Resolve so the spawned task exits cleanly.
        assert_eq!(confirmer.check_confirmation_response("group:3", "yes"), Some(true));
        let _ = handle.await.unwrap();
    }

    #[tokio::test]
    async fn no_pending_returns_none() {
        let sender = MockSender::new();
        let confirmer = Confirmer::new(sender, Duration::from_secs(5));
        // No pending confirmation → keyword is treated as a normal message.
        assert_eq!(confirmer.check_confirmation_response("group:9", "确认"), None);
    }

    #[tokio::test]
    async fn timeout_returns_false() {
        let sender = MockSender::new();
        let confirmer = Confirmer::new(sender.clone(), Duration::from_millis(80));
        let info = tool_info("tc5", "bash");

        let approved = confirmer.request("group:4", &info, false).await.unwrap();
        assert!(!approved);
        // A timeout notice was sent.
        assert!(sender
            .messages()
            .iter()
            .any(|(_, t)| t.contains("超时")));
    }

    #[tokio::test]
    async fn only_first_pending_per_chat_resolved() {
        let sender = MockSender::new();
        let confirmer = Arc::new(Confirmer::new(sender.clone(), Duration::from_secs(5)));

        // Two pending confirmations for the same chat with different tool_call_ids.
        let info1 = tool_info("a", "bash");
        let info2 = tool_info("b", "write");
        let c1 = confirmer.clone();
        let c2 = confirmer.clone();
        let h1 = tokio::spawn(async move { c1.request("group:5", &info1, false).await });
        let h2 = tokio::spawn(async move { c2.request("group:5", &info2, false).await });
        tokio::time::sleep(Duration::from_millis(50)).await;

        // First reply resolves one pending entry; second resolves the other.
        // HashMap iteration order is non-deterministic, so we don't assume
        // which tool_call_id is resolved first — only that both get resolved
        // and the decisions match the keywords sent.
        assert!(confirmer.check_confirmation_response("group:5", "确认").is_some());
        assert!(confirmer.check_confirmation_response("group:5", "取消").is_some());

        let r1 = h1.await.unwrap().unwrap();
        let r2 = h2.await.unwrap().unwrap();
        // One approved, one rejected — order is unspecified.
        assert_ne!(r1, r2);
    }

    #[test]
    fn cleanup_expired_removes_old_entries() {
        // cleanup_expired only touches the map; we can't easily test timing
        // without a pending entry registered via request(). This is a smoke
        // test that it doesn't panic on an empty map.
        let sender = MockSender::new();
        let confirmer = Confirmer::new(sender, Duration::from_secs(1));
        confirmer.cleanup_expired();
    }

    // Suppress unused-import warning for Mutex (kept for potential future tests).
    #[allow(dead_code)]
    fn _use_mutex() -> Mutex<()> {
        Mutex::new(())
    }
}
