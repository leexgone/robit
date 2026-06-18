//! Multi-session Bot orchestrator.
//!
//! NOTE: Full implementation lands in Phase 7.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use robit_agent::event::FrontendMessage;
use robit_agent::{Agent, SkillRegistry, ToolRegistry};
use robit_ai::config::RobitConfig;
use robit_ai::LlmClient;
use tokio::sync::Mutex;

use crate::adapter::PlatformAdapter;

/// Handle to a running Agent instance for one chat.
pub struct AgentHandle {
    pub message_tx: tokio::sync::mpsc::Sender<FrontendMessage>,
    pub session_id: String,
    pub last_active_at: Instant,
}

/// Core orchestrator for multi-session Bot operations.
///
/// NOTE: stub — full session lifecycle implemented in Phase 7.
pub struct ChatbotManager<T: PlatformAdapter> {
    #[allow(dead_code)]
    platform: T,
    #[allow(dead_code)]
    agents: Mutex<HashMap<String, AgentHandle>>,
    #[allow(dead_code)]
    config: RobitConfig,
    #[allow(dead_code)]
    working_dir: PathBuf,
    #[allow(dead_code)]
    llm_client: Arc<LlmClient>,
    #[allow(dead_code)]
    tool_registry: Arc<ToolRegistry>,
    #[allow(dead_code)]
    skill_registry: Arc<SkillRegistry>,
}

impl<T: PlatformAdapter> ChatbotManager<T> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        platform: T,
        config: RobitConfig,
        working_dir: PathBuf,
        llm_client: Arc<LlmClient>,
        tool_registry: Arc<ToolRegistry>,
        skill_registry: Arc<SkillRegistry>,
    ) -> Self {
        Self {
            platform,
            agents: Mutex::new(HashMap::new()),
            config,
            working_dir,
            llm_client,
            tool_registry,
            skill_registry,
        }
    }

    /// Main event loop. Phase 7 will implement platform event dispatch.
    pub async fn run(&self) -> robit_agent::error::Result<()> {
        // Phase 7: loop on self.platform.recv_event() and dispatch messages.
        Ok(())
    }
}
