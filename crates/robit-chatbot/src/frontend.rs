//! Per-session Frontend implementation for Bot platforms.
//!
//! NOTE: Full implementation lands in Phase 6.

use std::sync::Arc;

use async_trait::async_trait;
use robit_agent::error::Result;
use robit_agent::event::AgentEvent;
use robit_agent::frontend::Frontend;
use robit_agent::tool::ToolCallInfo;
use tokio::sync::Mutex;

use crate::adapter::{PlatformCaps, SendResult};
use crate::confirmer::Confirmer;

/// Abstracted message sending capability (platform-agnostic).
#[async_trait]
pub trait PlatformSender: Send + Sync {
    async fn send(&self, chat_id: &str, text: &str) -> Result<SendResult>;
    async fn edit(&self, chat_id: &str, msg_id: &str, text: &str) -> Result<()>;
    fn capabilities(&self) -> PlatformCaps;
}

/// Per-session Frontend trait implementation for Bot platforms.
///
/// Each chat (group or private) gets its own `ChatbotFrontend` instance.
/// TextDelta events are buffered and flushed in smart segments.
pub struct ChatbotFrontend {
    pub chat_id: String,
    pub platform_sender: Arc<dyn PlatformSender>,
    pub confirmer: Arc<Confirmer>,
    pub buffer: Mutex<String>,
    pub last_msg_id: Mutex<Option<String>>,
    pub progress_hint_sent: Mutex<bool>,
    pub auto_approve: bool,
}

#[async_trait]
impl Frontend for ChatbotFrontend {
    async fn on_event(&self, _event: AgentEvent) -> Result<()> {
        // Phase 6: handle TextDelta / ToolCallRequested / TurnComplete / Error / ...
        Ok(())
    }

    async fn request_tool_confirmation(&self, _info: &ToolCallInfo) -> Result<bool> {
        // Phase 6: delegate to confirmer.request()
        Ok(self.auto_approve)
    }
}
