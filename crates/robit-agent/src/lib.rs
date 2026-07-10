//! robit-agent: Agent runtime for the Robit framework.
//!
//! Provides the event-driven Agent loop, tool system, context management,
//! and the Frontend trait for pluggable UI frontends.

pub mod agent;
pub mod bootstrap;
pub mod context;
pub mod datetime;
pub mod error;
pub mod event;
pub mod frontend;
pub mod lock;
pub mod media;
pub mod prompt;
pub mod skill;
pub mod storage;
pub mod tool;

pub use agent::Agent;
pub use bootstrap::{
    bootstrap, create_tools_from_config, filter_skills_by_config, load_all_skills,
    log_skill_errors, BootstrapResult,
};
pub use error::AgentError;
pub use event::{AgentEvent, FrontendMessage, MediaAttachment, SessionId};
pub use lock::{DirectoryLock, LockError, LockInfo};
pub use media::{download_and_encode_base64, download_media, MediaError};
pub use frontend::{create_channels, AgentChannels, Frontend, FrontendChannels};
pub use skill::{Skill, SkillFrontmatter, SkillLoadError, SkillRegistry};
pub use storage::{load_chat_messages, message_to_chat_message};
pub use tool::load_skill::LoadSkillTool;
pub use tool::{Tool, ToolCallInfo, ToolContext, ToolRegistry, ToolResult};
