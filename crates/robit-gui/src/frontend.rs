use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};

use robit_agent::error::Result;
use robit_agent::event::AgentEvent;
use robit_agent::frontend::Frontend;
use robit_agent::tool::ToolCallInfo;

use crate::events::UiEvent;

/// Frontend trait implementation for the Tauri GUI.
///
/// Each session has its own GuiFrontend instance. Events are pushed via
/// a channel to a background task that emits Tauri events to the WebView.
/// Tool confirmations use oneshot channels stored in a shared AppState map.
pub struct GuiFrontend {
    /// Send UiEvents to the Tauri event bridge task.
    pub event_tx: mpsc::Sender<UiEvent>,

    /// Shared confirmations map from AppState (keyed by "session_id:tool_call_id").
    pub confirmations: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,

    /// The session this frontend belongs to.
    pub session_id: String,
}

#[async_trait]
impl Frontend for GuiFrontend {
    async fn on_event(&self, event: AgentEvent) -> Result<()> {
        let ui_event = match event {
            AgentEvent::TextDelta(delta) => UiEvent::TextDelta {
                session_id: self.session_id.clone(),
                delta,
            },
            AgentEvent::ToolCallRequested {
                tool_call_id,
                name,
                arguments,
            } => {
                let requires_confirm = matches!(
                    name.as_str(),
                    "bash" | "write" | "edit"
                );
                UiEvent::ToolCallRequested {
                    session_id: self.session_id.clone(),
                    tool_call_id,
                    name,
                    arguments,
                    requires_confirm,
                }
            }
            AgentEvent::ToolCallResult {
                tool_call_id,
                result,
            } => UiEvent::ToolCallResult {
                session_id: self.session_id.clone(),
                tool_call_id,
                content: result.content,
                is_error: result.is_error,
            },
            AgentEvent::TurnComplete => UiEvent::TurnComplete {
                session_id: self.session_id.clone(),
            },
            AgentEvent::Error(e) => UiEvent::Error {
                session_id: self.session_id.clone(),
                message: e.to_string(),
            },
            AgentEvent::SkillTriggered { name, description } => UiEvent::SkillTriggered {
                session_id: self.session_id.clone(),
                name,
                description,
            },
        };

        let _ = self.event_tx.send(ui_event).await;
        Ok(())
    }

    async fn request_tool_confirmation(&self, info: &ToolCallInfo) -> Result<bool> {
        let (tx, rx) = oneshot::channel();
        let key = format!("{}:{}", self.session_id, info.id);

        {
            let mut map = self.confirmations.lock().await;
            map.insert(key, tx);
        }

        let approved = rx.await.unwrap_or(false);

        // Cleanup
        let key = format!("{}:{}", self.session_id, info.id);
        {
            let mut map = self.confirmations.lock().await;
            map.remove(&key);
        }

        Ok(approved)
    }
}
