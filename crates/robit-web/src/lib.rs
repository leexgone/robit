//! robit-web: Web frontend bridge for the Robit framework.
//!
//! Lets a web host (axum, actix, warp, or anything that can stream JSON)
//! drive an [`robit_agent::Agent`] without re-implementing the host-agnostic
//! plumbing that every web UI needs:
//!
//! * [`WebEvent`] — the official JSON projection of `AgentEvent` (a stable
//!   wire schema for browsers: tagged `type` field, snake_case variants).
//! * [`WebFrontend`] — a [`robit_agent::Frontend`] implementation that fans
//!   events out on a `tokio::sync::broadcast` channel and turns
//!   `request_tool_confirmation` into a push/resolve handshake: the confirm
//!   request is broadcast to clients, and the agent task suspends on a
//!   oneshot slot until [`WebBridge::resolve_confirm`] (or a timeout /
//!   disconnect) answers it.
//! * [`WebBridge`] — the host-facing wiring: message injection
//!   ([`WebBridge::send_user_text`]) plus the subscribe/confirm endpoints.
//!
//! Wire format (one JSON object per event, discriminated by `type`):
//!
//! ```json
//! {"type":"text_delta","text":"…"}
//! {"type":"tool_call","tool_call_id":"…","name":"…","arguments":"…"}
//! {"type":"tool_result","tool_call_id":"…","is_error":false,"content":"…"}
//! {"type":"confirm_request","tool_call_id":"…","name":"…","arguments":"…"}
//! {"type":"turn_complete"}
//! ```
//!
//! SSE/WS encoding, keep-alive and transport-level concerns stay in the host
//! on purpose: this crate has no HTTP framework dependency. A typical axum
//! host maps `WebEvent` into SSE frames with `Event::default().json_data(e)`.
//!
//! Design note: single-slot confirmation matches the agent loop, which
//! requests tool confirmations serially. Multi-session hosts should create
//! one [`WebBridge`] per session.

pub mod bridge;
pub mod event;

pub use bridge::{FrontendGone, PendingConfirm, WebBridge, WebFrontend};
pub use event::WebEvent;
