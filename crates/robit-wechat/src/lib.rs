//! robit-wechat: WeChat iLink Bot platform frontend for the Robit agent.
//!
//! Implements [`robit_chatbot::PlatformAdapter`] for the official WeChat bot
//! channel (iLink AI Bot protocol, `ilinkai.weixin.qq.com`) via the
//! protocol-only `weixin-agent` SDK, and ships the `robit-wechat` binary.
//!
//! Login model: a one-time QR scan exchanges for a long-lived `bot_token`
//! (persisted in `wechat-state.json`); afterwards the client runs a long-poll
//! `getUpdates` loop — no public webhook needed, so intranet deployments work.

pub mod login;
pub mod platform;
pub mod state;
mod typing;

pub use platform::{WechatConfig, WechatPlatformAdapter};
