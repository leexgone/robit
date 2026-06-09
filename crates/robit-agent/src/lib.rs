//! robit-agent: Agent runtime for the Robit framework.
//!
//! Provides the event-driven Agent loop, tool system, context management,
//! and the Frontend trait for pluggable UI frontends.

pub mod agent;
pub mod context;
pub mod error;
pub mod event;
pub mod frontend;
pub mod prompt;
pub mod skill;
pub mod tool;

pub use agent::Agent;
pub use error::AgentError;
pub use event::{AgentEvent, FrontendMessage, SessionId};
pub use frontend::{create_channels, AgentChannels, Frontend, FrontendChannels};
pub use skill::{Skill, SkillFrontmatter, SkillLoadError, SkillRegistry};
pub use tool::{Tool, ToolCallInfo, ToolContext, ToolRegistry, ToolResult};
pub use tool::load_skill::LoadSkillTool;
