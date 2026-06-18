//! robit-qq: QQ Official Bot platform frontend for the Robit agent.
//!
//! Implements [`robit_chatbot::PlatformAdapter`] for the QQ Official Bot API
//! (WebSocket gateway + HTTP message sending) and ships the `robit-qq` binary.

pub mod platform;
pub mod protocol;

pub use platform::{QqConfig, QqPlatformAdapter};
