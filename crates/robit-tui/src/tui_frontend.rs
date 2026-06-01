//! TuiFrontend — implements the Frontend trait, forwarding events through channels.

use async_trait::async_trait;
use robit_agent::{AgentEvent, Frontend, ToolCallInfo};
use tokio::sync::{mpsc, oneshot};

/// A confirmation request sent from the Frontend to the TUI event loop.
pub struct ConfirmRequest {
    pub tool_info: ToolCallInfo,
    pub responder: oneshot::Sender<bool>,
}

/// TUI implementation of the Frontend trait.
/// All methods send data through channels — the TUI event loop handles rendering.
pub struct TuiFrontend {
    pub event_tx: mpsc::Sender<AgentEvent>,
    pub confirm_tx: mpsc::Sender<ConfirmRequest>,
}

#[async_trait]
impl Frontend for TuiFrontend {
    async fn on_event(&self, event: AgentEvent) -> robit_agent::error::Result<()> {
        let _ = self.event_tx.send(event).await;
        Ok(())
    }

    async fn request_tool_confirmation(
        &self,
        info: &ToolCallInfo,
    ) -> robit_agent::error::Result<bool> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .confirm_tx
            .send(ConfirmRequest {
                tool_info: info.clone(),
                responder: tx,
            })
            .await;
        match rx.await {
            Ok(approved) => Ok(approved),
            Err(_) => Ok(false),
        }
    }
}

