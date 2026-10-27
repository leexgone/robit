//! [`WebBridge`] / [`WebFrontend`] — channel plumbing between an Agent task
//! and one or more web clients.

use std::sync::Arc;

use async_trait::async_trait;
use robit_agent::error::Result;
use robit_agent::event::{AgentEvent, FrontendMessage};
use robit_agent::frontend::Frontend;
use robit_agent::tool::ToolCallInfo;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};

use crate::event::WebEvent;

/// A tool call awaiting user confirmation.
///
/// `tool_call_id` matches the id pushed in [`WebEvent::ConfirmRequest`];
/// `responder` completes the suspended `request_tool_confirmation` call.
pub struct PendingConfirm {
    pub tool_call_id: String,
    responder: oneshot::Sender<bool>,
}

/// Single-slot confirmation state shared between the frontend and the host.
/// The agent loop requests confirmations serially, so one slot per session
/// is sufficient; replacing the slot drops the old responder, which makes
/// the previous waiter resolve as denied.
type ConfirmSlot = Arc<Mutex<Option<PendingConfirm>>>;

/// The error returned when the agent loop's message channel is gone (the
/// `Agent::run` receiver was dropped, i.e. the agent task exited).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontendGone;

impl std::fmt::Display for FrontendGone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "agent message channel closed (agent loop exited)")
    }
}

impl std::error::Error for FrontendGone {}

/// A [`Frontend`] implementation that fans events out to web clients.
///
/// * `on_event` projects `AgentEvent` into [`WebEvent`] and broadcasts it.
///   A send failure (no current subscribers) is normal and ignored — nobody
///   is watching; the agent should keep running.
/// * `request_tool_confirmation` pushes a [`WebEvent::ConfirmRequest`],
///   suspends on a oneshot, and resolves with the host's answer
///   (see [`WebBridge::resolve_confirm`]). A dropped responder (replaced
///   request, [`WebBridge::reject_pending_confirms`], bridge teardown)
///   counts as **deny**.
pub struct WebFrontend {
    events: broadcast::Sender<WebEvent>,
    confirm_slot: ConfirmSlot,
}

impl WebFrontend {
    fn new(events: broadcast::Sender<WebEvent>, confirm_slot: ConfirmSlot) -> Arc<Self> {
        Arc::new(Self {
            events,
            confirm_slot,
        })
    }

    /// Subscribe to the projected event stream (one receiver per client/conn).
    pub fn subscribe(&self) -> broadcast::Receiver<WebEvent> {
        self.events.subscribe()
    }
}

#[async_trait]
impl Frontend for WebFrontend {
    async fn on_event(&self, event: AgentEvent) -> Result<()> {
        let _ = self.events.send(WebEvent::from_agent(&event));
        Ok(())
    }

    async fn request_tool_confirmation(&self, info: &ToolCallInfo) -> Result<bool> {
        let (tx, rx) = oneshot::channel();
        {
            let mut slot = self.confirm_slot.lock().await;
            *slot = Some(PendingConfirm {
                tool_call_id: info.id.clone(),
                responder: tx,
            });
        }
        let _ = self.events.send(WebEvent::ConfirmRequest {
            tool_call_id: info.id.clone(),
            name: info.name.clone(),
            arguments: info.arguments.clone(),
        });
        // Suspend until the host resolves over HTTP (or the request is
        // replaced / rejected / the bridge is dropped) — anything but an
        // explicit approval is a deny.
        match rx.await {
            Ok(approved) => Ok(approved),
            Err(_) => Ok(false),
        }
    }
}

/// Host-facing wiring for one agent session.
///
/// Build it before constructing the [`robit_agent::Agent`], hand
/// [`WebBridge::frontend`] to `Agent::new` and
/// [`WebBridge::take_message_rx`] to `Agent::run`; serve
/// [`WebBridge::subscribe`] as your event stream (SSE/WS) and route user
/// input / confirmations back through `send_*` / `resolve_confirm`.
///
/// One bridge per session. For multiple sessions, create one bridge (and
/// one Agent task) per session and key them in the host.
pub struct WebBridge {
    frontend: Arc<WebFrontend>,
    message_tx: mpsc::Sender<FrontendMessage>,
    message_rx: Option<mpsc::Receiver<FrontendMessage>>,
    confirm_slot: ConfirmSlot,
}

impl WebBridge {
    /// Create a bridge with explicit channel capacities.
    ///
    /// `event_capacity` bounds how far a slow SSE consumer may lag behind
    /// the agent before frames are dropped (each client that falls behind
    /// gets a `Lagged` error from its receiver — surface it to the client
    /// rather than killing the stream). `message_capacity` bounds buffered
    /// inbound user messages.
    pub fn new(event_capacity: usize, message_capacity: usize) -> Self {
        let (events, _) = broadcast::channel(event_capacity.max(1));
        let (message_tx, message_rx) = mpsc::channel(message_capacity.max(1));
        let confirm_slot: ConfirmSlot = Arc::new(Mutex::new(None));
        let frontend = WebFrontend::new(events, confirm_slot.clone());
        Self {
            frontend,
            message_tx,
            message_rx: Some(message_rx),
            confirm_slot,
        }
    }

    /// The `Frontend` implementation to give to `Agent::new`.
    pub fn frontend(&self) -> Arc<WebFrontend> {
        self.frontend.clone()
    }

    /// The `FrontendMessage` receiver to give to `Agent::run`. Takes it once;
    /// calling again panics (the agent loop needs sole ownership).
    pub fn take_message_rx(&mut self) -> mpsc::Receiver<FrontendMessage> {
        self.message_rx
            .take()
            .expect("WebBridge::take_message_rx called more than once")
    }

    /// Subscribe to the projected event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<WebEvent> {
        self.frontend.subscribe()
    }

    /// Inject a user message (the "message injection" half of the contract).
    pub async fn send_user_text(
        &self,
        text: impl Into<String>,
    ) -> std::result::Result<(), FrontendGone> {
        self.send_message(FrontendMessage::UserInput {
            text: text.into(),
            attachments: vec![],
        })
        .await
    }

    /// Inject any frontend message (`UserInput`, `Cancel`, `CancelTask`, …).
    pub async fn send_message(
        &self,
        msg: FrontendMessage,
    ) -> std::result::Result<(), FrontendGone> {
        self.message_tx.send(msg).await.map_err(|_| FrontendGone)
    }

    /// Answer a pending confirmation. Returns `false` when there is no
    /// pending request matching `tool_call_id` (stale or wrong id).
    pub async fn resolve_confirm(&self, tool_call_id: &str, approved: bool) -> bool {
        let mut guard = self.confirm_slot.lock().await;
        let matched = guard
            .as_ref()
            .map(|p| p.tool_call_id == tool_call_id)
            .unwrap_or(false);
        if matched {
            if let Some(pending) = guard.take() {
                let _ = pending.responder.send(approved);
            }
        }
        matched
    }

    /// Reject whatever is pending (host shutdown, session teardown, a
    /// "deny all" control). Returns whether there was a pending request.
    pub async fn reject_pending_confirms(&self) -> bool {
        self.confirm_slot.lock().await.take().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str) -> ToolCallInfo {
        ToolCallInfo {
            id: id.to_string(),
            name: "bash".to_string(),
            arguments: "{}".to_string(),
        }
    }

    #[tokio::test]
    async fn on_event_broadcasts_projected_wire_event() {
        let bridge = WebBridge::new(16, 16);
        let mut rx = bridge.subscribe();
        let fe = bridge.frontend();
        fe.on_event(AgentEvent::TextDelta("你好".into())).await.unwrap();
        assert_eq!(
            rx.recv().await.unwrap(),
            WebEvent::TextDelta { text: "你好".into() }
        );
    }

    #[tokio::test]
    async fn confirm_roundtrip_approve() {
        let bridge = WebBridge::new(16, 16);
        let mut rx = bridge.subscribe();
        let fe = bridge.frontend();
        let handle =
            tokio::spawn(async move { fe.request_tool_confirmation(&info("tc1")).await });
        let first = rx.recv().await.unwrap();
        assert_eq!(
            first,
            WebEvent::ConfirmRequest {
                tool_call_id: "tc1".into(),
                name: "bash".into(),
                arguments: "{}".into(),
            }
        );
        assert!(bridge.resolve_confirm("tc1", true).await);
        assert_eq!(handle.await.unwrap().unwrap(), true);
    }

    #[tokio::test]
    async fn confirm_wrong_id_does_not_resolve() {
        let bridge = WebBridge::new(16, 16);
        let mut rx = bridge.subscribe();
        let fe = bridge.frontend();
        let handle =
            tokio::spawn(async move { fe.request_tool_confirmation(&info("tc1")).await });
        rx.recv().await.unwrap();
        assert!(!bridge.resolve_confirm("nope", true).await);
        // still pending — correct id resolves it
        assert!(bridge.resolve_confirm("tc1", false).await);
        assert_eq!(handle.await.unwrap().unwrap(), false);
    }

    #[tokio::test]
    async fn replaced_request_denies_the_old_waiter() {
        let bridge = WebBridge::new(16, 16);
        let mut rx = bridge.subscribe();
        let fe1 = bridge.frontend();
        let fe2 = bridge.frontend();
        let old = tokio::spawn(async move { fe1.request_tool_confirmation(&info("tc1")).await });
        rx.recv().await.unwrap();
        let new = tokio::spawn(async move { fe2.request_tool_confirmation(&info("tc2")).await });
        let second = rx.recv().await.unwrap();
        assert!(matches!(
            second,
            WebEvent::ConfirmRequest { ref tool_call_id, .. } if tool_call_id == "tc2"
        ));
        // old slot was replaced: its waiter resolves as deny without any answer
        assert_eq!(old.await.unwrap().unwrap(), false);
        assert!(bridge.resolve_confirm("tc2", true).await);
        assert_eq!(new.await.unwrap().unwrap(), true);
    }

    #[tokio::test]
    async fn reject_pending_denies_and_reports() {
        let bridge = WebBridge::new(16, 16);
        let mut rx = bridge.subscribe();
        let fe = bridge.frontend();
        let handle =
            tokio::spawn(async move { fe.request_tool_confirmation(&info("tc1")).await });
        rx.recv().await.unwrap();
        assert!(bridge.reject_pending_confirms().await);
        assert!(!bridge.reject_pending_confirms().await);
        assert_eq!(handle.await.unwrap().unwrap(), false);
    }

    #[tokio::test]
    async fn send_user_text_fails_after_receiver_dropped() {
        let mut bridge = WebBridge::new(16, 16);
        assert!(bridge.send_user_text("hi").await.is_ok());
        let rx = bridge.take_message_rx();
        drop(rx); // agent loop exited
        assert_eq!(bridge.send_user_text("hi").await, Err(FrontendGone));
    }
}
