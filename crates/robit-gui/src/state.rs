use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use robit_agent::agent::Agent;
use robit_agent::event::{FrontendMessage, SessionId};
use robit_agent::skill::SkillRegistry;
use robit_agent::storage::{resolve_db_path, load_chat_messages};
use robit_agent::tool::ToolRegistry;
use robit_agent::{bootstrap, log_skill_errors};
use robit_ai::config::RobitConfig;
use robit_ai::LlmClient;
use rusqlite::Connection;
use tauri::Emitter;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::db;
use crate::events::{ConfigInfo, SessionInfo};
use crate::frontend::GuiFrontend;

fn resolve_working_dir(working_dir: Option<PathBuf>) -> Result<PathBuf, String> {
    match working_dir {
        Some(path) => {
            if !path.exists() {
                return Err(format!(
                    "Working directory does not exist: {}",
                    path.display()
                ));
            }
            if !path.is_dir() {
                return Err(format!("Path is not a directory: {}", path.display()));
            }
            // Canonicalize to get absolute path (resolves symlinks, etc.)
            std::fs::canonicalize(path)
                .map_err(|e| format!("Failed to resolve working directory path: {}", e))
        }
        None => {
            std::env::current_dir().map_err(|e| format!("Failed to get current directory: {}", e))
        }
    }
}

fn should_use_global_storage(config: &RobitConfig, cli_global_storage: bool) -> bool {
    cli_global_storage
        || config
            .app
            .as_ref()
            .and_then(|a| a.global_storage)
            .unwrap_or(false)
}

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

    /// Total skills loaded (including disabled ones).
    pub total_skills: usize,

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
        llm_client: Arc<LlmClient>,
        config: RobitConfig,
        working_dir: Option<PathBuf>,
        cli_global_storage: bool,
    ) -> Result<Self, String> {
        // Resolve and validate working directory
        let working_dir = resolve_working_dir(working_dir)?;
        let global_storage = should_use_global_storage(&config, cli_global_storage);
        let db_path = resolve_db_path(&working_dir, global_storage)
            .map_err(|e| e.to_string())?;

        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create DB dir: {}", e))?;
        }

        tracing::info!(
            db_path = %db_path.display(),
            global_storage,
            "Using session database"
        );

        let conn =
            Connection::open(&db_path).map_err(|e| format!("Failed to open database: {}", e))?;

        db::init_db(&conn).map_err(|e| format!("Failed to init database: {}", e))?;

        let db = Arc::new(Mutex::new(conn));

        let auto_approve = config
            .app
            .as_ref()
            .and_then(|a| a.auto_approve)
            .unwrap_or(false);

        // ContextConfig doesn't implement Clone - skip MVP
        let context_config: Option<robit_ai::config::ContextConfig> = None;

        let context_window = llm_client.resolved().context_window;

        // Bootstrap skills and tools
        let base_tool_names = ["read", "bash", "write", "edit"];
        let bootstrap_result = bootstrap(&config, &working_dir, &base_tool_names);
        log_skill_errors(&bootstrap_result.skill_load_errors);

        let skill_registry = bootstrap_result.skill_registry;
        let tool_registry = bootstrap_result.tool_registry;
        let total_skills = bootstrap_result.total_skills_loaded;

        Ok(Self {
            db,
            llm_client,
            tool_registry,
            skill_registry,
            total_skills,
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
        let mut sessions = db::list_sessions(&db, None).map_err(|e| format!("DB error: {}", e))?;

        let agents = self.agents.lock().await;
        for session in &mut sessions {
            if let Some(handle) = agents.get(&session.id) {
                session.status = match handle.status {
                    AgentStatus::Idle => "idle",
                    AgentStatus::Ready => "ready",
                    AgentStatus::Running => "running",
                }
                .to_string();
            }
        }

        Ok(sessions)
    }

    /// Build ConfigInfo for the frontend.
    pub fn config_info(&self) -> ConfigInfo {
        let tools = self.tool_registry.tool_names();
        let mut info = crate::config::build_config_info(&self.config, &self.working_dir);
        info.tools_enabled = tools.len();
        info.tools_total = tools.len();
        info.tool_names = tools.into_iter().map(str::to_string).collect();
        info.skills_enabled = self.skill_registry.count();
        info.skills_total = self.total_skills;
        info.skill_names = self
            .skill_registry
            .skill_names()
            .into_iter()
            .map(str::to_string)
            .collect();
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

        // Load history messages from database
        let db = self.db.lock().await;
        let history_messages = load_chat_messages(&db, session_id).unwrap_or_default();
        drop(db);

        let working_dir = self.working_dir.clone();
        let auto_approve = self.auto_approve;
        let llm_client = Arc::clone(&self.llm_client);
        let tools = Arc::clone(&self.tool_registry);
        let skills = Arc::clone(&self.skill_registry);
        let context_window = self.context_window;

        // Parse session_id string to SessionId
        let session_id_obj = SessionId::from(session_id.to_string());

        let agent = Agent::with_history(
            llm_client,
            tools,
            skills,
            gui_frontend,
            None,
            context_window,
            working_dir,
            auto_approve,
            std::collections::HashMap::new(),
            session_id_obj,
            history_messages,
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

            // Helper: save accumulated buffer as assistant message
            let save_buffer = |db: &tokio::sync::MutexGuard<'_, rusqlite::Connection>, buffer: &str, sid: &str| {
                if !buffer.is_empty() {
                    let _ = crate::db::insert_message(
                        db,
                        sid,
                        "assistant",
                        buffer,
                        None,
                        None,
                        None,
                    );
                    let _ = crate::db::touch_session(db, sid);
                }
            };

            // TextDelta batching: accumulate deltas and send in batches
            let mut delta_buffer = String::new();
            let mut last_delta_send = tokio::time::Instant::now();
            const DELTA_BATCH_MS: u64 = 50; // Batch deltas every 50ms

            loop {
                tokio::select! {
                    // Receive next event
                    event = event_rx.recv() => {
                        match event {
                            Some(event) => {
                                match &event {
                                    crate::events::UiEvent::TextDelta { delta, .. } => {
                                        delta_buffer.push_str(delta);
                                        buffer.push_str(delta);
                                    }
                                    crate::events::UiEvent::ToolCallRequested {
                                        tool_call_id,
                                        name,
                                        arguments,
                                        requires_confirm,
                                        ..
                                    } => {
                                        // Flush any pending deltas first
                                        if !delta_buffer.is_empty() {
                                            let flush_event = crate::events::UiEvent::TextDelta {
                                                session_id: sid_clone.clone(),
                                                delta: std::mem::take(&mut delta_buffer),
                                            };
                                            let _ = app_handle_clone.emit("agent-event", &flush_event);
                                            last_delta_send = tokio::time::Instant::now();
                                        }

                                        // Save accumulated text as assistant message BEFORE tool call
                                        if !buffer.is_empty() {
                                            let db = db_clone.lock().await;
                                            save_buffer(&db, &buffer, &sid_clone);
                                            buffer.clear();
                                        }

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
                                        let _ = app_handle_clone.emit("agent-event", &event);
                                    }
                                    crate::events::UiEvent::ToolCallResult {
                                        tool_call_id,
                                        content,
                                        is_error,
                                        ..
                                    } => {
                                        // Flush any pending deltas first
                                        if !delta_buffer.is_empty() {
                                            let flush_event = crate::events::UiEvent::TextDelta {
                                                session_id: sid_clone.clone(),
                                                delta: std::mem::take(&mut delta_buffer),
                                            };
                                            let _ = app_handle_clone.emit("agent-event", &flush_event);
                                            last_delta_send = tokio::time::Instant::now();
                                        }

                                        // Update tool message with result - merge with existing data
                                        let db = db_clone.lock().await;
                                        // First try to get the existing message to preserve tool_name and arguments
                                        let messages = crate::db::get_messages(&db, &sid_clone).unwrap_or_default();
                                        let existing_tool_info = messages.iter()
                                            .find(|m| m.tool_call_id.as_deref() == Some(tool_call_id))
                                            .and_then(|m| m.tool_info.clone());

                                        let tool_info = if let Some(mut info) = existing_tool_info {
                                            // Merge with existing data
                                            if let serde_json::Value::Object(ref mut map) = info {
                                                map.insert("status".to_string(), if *is_error { "error".into() } else { "success".into() });
                                                map.insert("output".to_string(), content.to_string().into());
                                            }
                                            info
                                        } else {
                                            // Fallback to minimal data if no existing info
                                            serde_json::json!({
                                                "tool_call_id": tool_call_id,
                                                "status": if *is_error { "error" } else { "success" },
                                                "output": content,
                                                "name": "",
                                                "arguments": "",
                                                "requires_confirm": false,
                                            })
                                        };
                                        let tool_info_str = serde_json::to_string(&tool_info).unwrap_or_default();
                                        let _ = crate::db::update_tool_message(
                                            &db,
                                            &sid_clone,
                                            tool_call_id,
                                            &tool_info_str,
                                        );
                                        let _ = app_handle_clone.emit("agent-event", &event);
                                    }
                                    crate::events::UiEvent::TurnComplete { .. } => {
                                        // Flush any pending deltas first
                                        if !delta_buffer.is_empty() {
                                            let flush_event = crate::events::UiEvent::TextDelta {
                                                session_id: sid_clone.clone(),
                                                delta: std::mem::take(&mut delta_buffer),
                                            };
                                            let _ = app_handle_clone.emit("agent-event", &flush_event);
                                        }

                                        // Save assistant message to database
                                        if !buffer.is_empty() {
                                            let db = db_clone.lock().await;
                                            save_buffer(&db, &buffer, &sid_clone);
                                            buffer.clear();
                                        }
                                        let _ = app_handle_clone.emit("agent-event", &event);
                                    }
                                    _ => {
                                        // Flush any pending deltas first
                                        if !delta_buffer.is_empty() {
                                            let flush_event = crate::events::UiEvent::TextDelta {
                                                session_id: sid_clone.clone(),
                                                delta: std::mem::take(&mut delta_buffer),
                                            };
                                            let _ = app_handle_clone.emit("agent-event", &flush_event);
                                            last_delta_send = tokio::time::Instant::now();
                                        }
                                        let _ = app_handle_clone.emit("agent-event", &event);
                                    }
                                }
                            }
                            None => {
                                // Channel closed, flush any remaining deltas
                                if !delta_buffer.is_empty() {
                                    let flush_event = crate::events::UiEvent::TextDelta {
                                        session_id: sid_clone.clone(),
                                        delta: std::mem::take(&mut delta_buffer),
                                    };
                                    let _ = app_handle_clone.emit("agent-event", &flush_event);
                                }
                                // Save any remaining buffer
                                if !buffer.is_empty() {
                                    let db = db_clone.lock().await;
                                    save_buffer(&db, &buffer, &sid_clone);
                                }
                                break;
                            }
                        }
                    }

                    // Periodic flush of deltas
                    _ = tokio::time::sleep_until(last_delta_send + tokio::time::Duration::from_millis(DELTA_BATCH_MS)), if !delta_buffer.is_empty() => {
                        let flush_event = crate::events::UiEvent::TextDelta {
                            session_id: sid_clone.clone(),
                            delta: std::mem::take(&mut delta_buffer),
                        };
                        let _ = app_handle_clone.emit("agent-event", &flush_event);
                        last_delta_send = tokio::time::Instant::now();
                    }
                }
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
