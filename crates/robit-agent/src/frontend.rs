//! Frontend trait — abstract interface between Agent and UI.
//!
//! The Agent doesn't know whether the frontend is a TUI, Feishu bot, or QQ bot.
//! It only interacts through this trait.

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::error::Result;
use crate::event::{AgentEvent, FrontendMessage};
use crate::tool::ToolCallInfo;

/// Abstract frontend interface. Implementations handle rendering events and
/// collecting user input.
#[async_trait]
pub trait Frontend: Send + Sync {
    /// Agent pushes events to the frontend (text deltas, tool calls, errors, etc.).
    async fn on_event(&self, event: AgentEvent) -> Result<()>;

    /// Request user confirmation for a tool call. Blocks until user responds.
    /// Returns `true` if the user approved, `false` if rejected.
    async fn request_tool_confirmation(&self, info: &ToolCallInfo) -> Result<bool>;
}

/// Channel pair for Agent <-> Frontend communication.
///
/// The Agent holds `event_tx` (to send events) and `message_rx` (to receive user messages).
/// The Frontend holds the other ends.
pub struct AgentChannels {
    /// Agent uses this to send events to the Frontend.
    pub event_tx: mpsc::Sender<AgentEvent>,
    /// Agent uses this to receive messages from the Frontend.
    pub message_rx: mpsc::Receiver<FrontendMessage>,
}

/// Frontend-side channel ends.
pub struct FrontendChannels {
    /// Frontend uses this to receive events from the Agent.
    pub event_rx: mpsc::Receiver<AgentEvent>,
    /// Frontend uses this to send messages to the Agent.
    pub message_tx: mpsc::Sender<FrontendMessage>,
}

/// Create a matched pair of Agent and Frontend channels.
pub fn create_channels(
    event_buffer: usize,
    message_buffer: usize,
) -> (AgentChannels, FrontendChannels) {
    let (event_tx, event_rx) = mpsc::channel(event_buffer);
    let (message_tx, message_rx) = mpsc::channel(message_buffer);

    let agent_channels = AgentChannels {
        event_tx,
        message_rx,
    };

    let frontend_channels = FrontendChannels {
        event_rx,
        message_tx,
    };

    (agent_channels, frontend_channels)
}
