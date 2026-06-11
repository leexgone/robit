use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use robit_agent::agent::Agent;
use robit_agent::event::{FrontendMessage, SessionId};
use robit_agent::skill::SkillRegistry;
use tauri::Emitter;
use robit_agent::tool::bash::BashTool;
use robit_agent::tool::edit::EditTool;
use robit_agent::tool::load_skill::LoadSkillTool;
use robit_agent::tool::read::ReadTool;
use robit_agent::tool::write::WriteTool;
use robit_agent::ToolRegistry;
use robit_ai::config::RobitConfig;
use robit_ai::LlmClient;
use rusqlite::Connection;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::db;
use crate::events::{ConfigInfo, SessionInfo};
use crate::frontend::GuiFrontend;

/// Agent run status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Ready,
    Running,
}

/// Handle to a running Agent instance for one session.
pub struct AgentHandle {
    /// Send messages to the Agent loop.
    pub message_tx: mpsc::Sender<FrontendMessage>,
    /// Current status.
    pub status: AgentStatus,
    /// Cancel token for interrupting a running Agent.
    pub cancel_token: CancellationToken,
}

/// Application state managed by Tauri.
pub struct AppState {
    /// SQLite connection (Mutex-protected, single connection).
    pub db: Arc<Mutex<Connection>>,

    /// Shared LLM client (reused across all sessions).
    pub llm_client: Arc<LlmClient>,

    /// Shared tool registry (reused across all sessions).
    pub tool_registry: Arc<ToolRegistry>,

    /// Shared skill registry.
    pub skill_registry: Arc<SkillRegistry>,

    /// Active Agent handles, keyed by session ID.
    pub agents: Mutex<HashMap<SessionId, AgentHandle>>,

    /// Currently active session ID.
    pub active_session: Mutex<Option<SessionId>>,

    /// Loaded configuration.
    pub config: RobitConfig,

    /// Working directory.
    pub working_dir: PathBuf,

    /// Auto-approve all tool calls.
    pub auto_approve: bool,

    /// Context config.
    pub context_config: Option<robit_ai::config::ContextConfig>,

    /// Context window from resolved model.
    pub context_window: Option<u64>,

    /// Pending tool confirmation responders, keyed by "session_id:tool_call_id".
    pub confirmations: Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>>,
}

impl AppState {
    /// Create a new AppState, initializing the database.
    pub fn new(
        db_path: PathBuf,
        llm_client: Arc<LlmClient>,
        config: RobitConfig,
    ) -> Result<Self, String> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create DB dir: {}", e))?;
        }

        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open database: {}", e))?;

        db::init_db(&conn).map_err(|e| format!("Failed to init database: {}", e))?;

        let db = Arc::new(Mutex::new(conn));

        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let auto_approve = config
            .app
            .as_ref()
            .and_then(|a| a.auto_approve)
            .unwrap_or(false);

        // ContextConfig doesn't implement Clone - skip MVP
        let context_config: Option<robit_ai::config::ContextConfig> = None;

        let context_window = llm_client.resolved().context_window;

        // Load skills
        let global_skills_dir = dirs::home_dir().map(|h| h.join(".robit/skills"));
        let project_skills_dir = Some(working_dir.join(".robit/skills"));

        let (skills, skill_errors) = robit_agent::skill::loader::load_skills(
            global_skills_dir,
            project_skills_dir,
        );

        for err in &skill_errors {
            tracing::warn!("Skill load error: {:?}", err);
        }

        let enabled_skills = config
            .app
            .as_ref()
            .and_then(|a| a.enabled_skills.as_ref());

        let filtered_skills: Vec<_> = match enabled_skills {
            Some(list) => skills
                .into_iter()
                .filter(|s| list.contains(&s.frontmatter.name))
                .collect(),
            None => skills,
        };

        let base_tool_names = ["read", "bash", "write", "edit"];
        let skill_registry = Arc::new(SkillRegistry::new(filtered_skills, &base_tool_names));

        let tool_registry = Arc::new(create_tools(&config, Arc::clone(&skill_registry)));

        Ok(Self {
            db,
            llm_client,
            tool_registry,
            skill_registry,
            agents: Mutex::new(HashMap::new()),
            active_session: Mutex::new(None),
            config,
            working_dir,
            auto_approve,
            context_config,
            context_window,
            confirmations: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Get the session list from the database, merging with in-memory Agent statuses.
    pub async fn session_list(&self) -> Result<Vec<SessionInfo>, String> {
        let db = self.db.lock().await;
        let mut sessions = db::list_sessions(&db).map_err(|e| format!("DB error: {}", e))?;

        let agents = self.agents.lock().await;
        for session in &mut sessions {
            if let Some(handle) = agents.get(&session.id) {
                session.status = match handle.status {
                    AgentStatus::Idle => "idle",
                    AgentStatus::Ready => "ready",
                    AgentStatus::Running => "running",
                }.to_string();
            }
        }

        Ok(sessions)
    }

    /// Build ConfigInfo for the frontend.
    pub fn config_info(&self) -> ConfigInfo {
        let tools = self.tool_registry.tool_names();
        let mut info = crate::config::build_config_info(&self.config);
        info.tools_enabled = tools.len();
        info.tools_total = tools.len();
        info
    }

    /// Create an Agent for a session and spawn its background task.
    pub async fn spawn_agent(
        &self,
        session_id: &str,
        app_handle: &tauri::AppHandle,
    ) -> Result<AgentHandle, String> {
        let (event_tx, mut event_rx) = mpsc::channel::<crate::events::UiEvent>(64);
        let (message_tx, message_rx) = mpsc::channel::<FrontendMessage>(16);

        let confirmations = Arc::clone(&self.confirmations);
        let auto_approve = self.auto_approve;
        let gui_frontend = Arc::new(GuiFrontend {
            event_tx,
            confirmations,
            session_id: session_id.to_string(),
            auto_approve,
        });

        let working_dir = self.working_dir.clone();
        let auto_approve = self.auto_approve;
        let llm_client = Arc::clone(&self.llm_client);
        let tools = Arc::clone(&self.tool_registry);
        let skills = Arc::clone(&self.skill_registry);
        let context_window = self.context_window;

        let agent = Agent::new(
            llm_client,
            tools,
            skills,
            gui_frontend,
            None,
            context_window,
            working_dir,
            auto_approve,
        );

        let cancel_token = CancellationToken::new();
        let sid = session_id.to_string();

        // Spawn the Agent loop in a background task
        tokio::spawn(async move {
            agent.run(message_rx).await;
            tracing::info!("Agent task ended for session {}", sid);
        });

        // Spawn event bridge: receives UiEvents and emits to Tauri frontend
        let app_handle_clone = app_handle.clone();
        let sid_clone = session_id.to_string();
        let db_clone = Arc::clone(&self.db);
        tokio::spawn(async move {
            let mut buffer = String::new();
            while let Some(event) = event_rx.recv().await {
                match &event {
                    crate::events::UiEvent::TextDelta { delta, .. } => {
                        buffer.push_str(delta);
                    }
                    crate::events::UiEvent::ToolCallRequested { tool_call_id, name, arguments, requires_confirm } => {
                        // Save tool call request to database
                        let tool_info = serde_json::json!({
                            "tool_call_id": tool_call_id,
                            "name": name,
                            "arguments": arguments,
                            "status": if *requires_confirm { "awaiting_confirmation" } else { "running" },
                            "requires_confirm": requires_confirm,
                        });
                        let tool_info_str = serde_json::to_string(&tool_info).unwrap_or_default();
                        let db = db_clone.lock().await;
                        let _ = crate::db::insert_message(
                            &db,
                            &sid_clone,
                            "tool",
                            arguments,
                            Some(name),
                            Some(tool_call_id),
                            Some(&tool_info_str),
                        );
                        let _ = crate::db::touch_session(&db, &sid_clone);
                    }
                    crate::events::UiEvent::ToolCallResult { tool_call_id, content, is_error } => {
                        // Update tool message with result
                        // First, get the current tool_info if it exists
                        let db = db_clone.lock().await;
                        let tool_info = serde_json::json!({
                            "tool_call_id": tool_call_id,
                            "status": if *is_error { "error" } else { "success" },
                            "output": content,
                        });
                        let tool_info_str = serde_json::to_string(&tool_info).unwrap_or_default();
                        let _ = crate::db::update_tool_message(&db, &sid_clone, tool_call_id, &tool_info_str);
                    }
                    crate::events::UiEvent::TurnComplete { .. } => {
                        // Save assistant message to database
                        if !buffer.is_empty() {
                            let db = db_clone.lock().await;
                            let _ = crate::db::insert_message(
                                &db,
                                &sid_clone,
                                "assistant",
                                &buffer,
                                None,
                                None,
                            );
                            let _ = crate::db::touch_session(&db, &sid_clone);
                            buffer.clear();
                        }
                    }
                    _ => {}
                }
                let _ = app_handle_clone.emit("agent-event", &event);
            }
            tracing::info!("Event bridge ended for session {}", sid_clone);
        });

        // Spawn cancel watcher
        let ct_watcher = cancel_token.clone();
        let msg_tx = message_tx.clone();
        tokio::spawn(async move {
            ct_watcher.cancelled().await;
            let _ = msg_tx.send(FrontendMessage::Cancel).await;
        });

        Ok(AgentHandle {
            message_tx,
            status: AgentStatus::Ready,
            cancel_token,
        })
    }
}

/// Build the tool registry (same as robit-tui).
fn create_tools(
    config: &RobitConfig,
    skills: Arc<SkillRegistry>,
) -> ToolRegistry {
    let mut tools = ToolRegistry::new();
    let context_config = config.app.as_ref().and_then(|a| a.context.as_ref());
    let max_lines = context_config.and_then(|c| c.max_output_lines).unwrap_or(500);
    let max_bytes = context_config
        .and_then(|c| c.max_output_bytes)
        .unwrap_or(51200);
    tools.register(ReadTool::new(max_lines, max_bytes));
    tools.register(BashTool::new(max_bytes));
    tools.register(WriteTool::new());
    tools.register(EditTool::new());
    tools.register(LoadSkillTool::new(skills));
    tools
}
