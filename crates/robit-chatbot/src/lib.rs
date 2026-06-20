//! robit-chatbot: Multi-session Bot infrastructure for the Robit framework.
//!
//! Provides the platform-agnostic `PlatformAdapter` trait, `ChatbotManager`
//! (multi-session orchestration), `ChatbotFrontend` (per-session Frontend
//! implementation with streaming), `Confirmer` (inline tool confirmation),
//! and a Markdown sanitizer. Platform crates (e.g. `robit-qq`) implement
//! `PlatformAdapter` and reuse everything else.

pub mod adapter;
pub mod confirmer;
pub mod frontend;
pub mod manager;
pub mod markdown;

pub use adapter::{
    ChatMessage, ChatType, MarkdownFeatures, MediaAttachment, PlatformAdapter, PlatformCaps,
    PlatformEvent, SendResult, SenderInfo, UploadResult,
};
pub use confirmer::{ConfirmKeywords, Confirmer};
pub use frontend::{ChatbotFrontend, PlatformSender};
pub use manager::{AgentHandle, ChatbotManager, ManagerError};
