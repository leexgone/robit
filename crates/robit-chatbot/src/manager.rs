//! Multi-session Bot orchestrator.
//!
//! [`ChatbotManager<T>`] is the core of `robit-chatbot`. It connects to a
//! platform via [`PlatformAdapter`](crate::adapter::PlatformAdapter), receives
//! chat events, and routes each message to an independent Agent session — one
//! Agent per chat, matching the `robit-gui` pattern. Sessions are persisted to
//! SQLite keyed by platform `chat_id`, so a chat that messages the bot again
//! after its in-memory Agent expired gets a fresh session backed by the same
//! DB record.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use robit_agent::event::FrontendMessage;
use robit_agent::storage::{self, resolve_db_path};
use robit_agent::tool::ToolCallInfo;
use robit_agent::{Agent, AgentError, SkillRegistry, ToolRegistry};
use robit_ai::config::RobitConfig;
use robit_ai::LlmClient;
use rusqlite::Connection;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use crate::adapter::{ChatMessage, PlatformAdapter, PlatformCaps, PlatformEvent, SendResult};
use crate::confirmer::{ConfirmKeywords, Confirmer};
use crate::frontend::{ChatbotFrontend, PlatformSender};

/// How often the cleanup loop scans for idle sessions.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(300); // 5 minutes

/// Handle to a running Agent instance for one chat.
pub struct AgentHandle {
    /// Send messages (user input) to the Agent loop.
    pub message_tx: mpsc::Sender<FrontendMessage>,
    pub session_id: String,
    pub last_active_at: Instant,
}

/// Bridge from a concrete `PlatformAdapter` to the platform-agnostic
/// `PlatformSender` trait used by `ChatbotFrontend` and `Confirmer`.
struct PlatformSenderBridge<T: PlatformAdapter> {
    platform: Arc<T>,
    caps: PlatformCaps,
}

#[async_trait]
impl<T: PlatformAdapter> PlatformSender for PlatformSenderBridge<T> {
    async fn send(&self, chat_id: &str, text: &str) -> robit_agent::error::Result<SendResult> {
        self.platform.send_message(chat_id, text).await
    }
    async fn edit(&self, chat_id: &str, msg_id: &str, text: &str) -> robit_agent::error::Result<()> {
        self.platform.edit_message(chat_id, msg_id, text).await
    }
    fn capabilities(&self) -> PlatformCaps {
        self.caps.clone()
    }
}

/// Core orchestrator for multi-session Bot operations.
pub struct ChatbotManager<T: PlatformAdapter> {
    /// The connected platform adapter, shared with the sender bridge.
    platform: Arc<T>,
    /// Active Agent instances, keyed by chat_id.
    agents: Mutex<HashMap<String, AgentHandle>>,
    /// SQLite connection for session persistence.
    db: Arc<Mutex<Connection>>,
    config: RobitConfig,
    working_dir: PathBuf,
    llm_client: Arc<LlmClient>,
    tool_registry: Arc<ToolRegistry>,
    skill_registry: Arc<SkillRegistry>,
    /// Shared platform sender (wraps the adapter).
    platform_sender: Arc<dyn PlatformSender>,
    /// Shared tool confirmation coordinator.
    confirmer: Arc<Confirmer>,
    auto_approve: bool,
    context_window: Option<u64>,
    /// Idle session expiry.
    session_timeout: Duration,
}

impl<T: PlatformAdapter> ChatbotManager<T> {
    /// Create a new `ChatbotManager`.
    ///
    /// Opens (or creates) the session database and initializes the shared
    /// `Confirmer` and platform sender bridge. `platform` must already be
    /// connected (the platform crate owns connection lifecycle).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        platform: Arc<T>,
        config: RobitConfig,
        working_dir: PathBuf,
        llm_client: Arc<LlmClient>,
        tool_registry: Arc<ToolRegistry>,
        skill_registry: Arc<SkillRegistry>,
    ) -> Result<Self, ManagerError> {
        let caps = T::capabilities();
        let platform_sender: Arc<dyn PlatformSender> = Arc::new(PlatformSenderBridge {
            platform: Arc::clone(&platform),
            caps: caps.clone(),
        });

        // Resolve bot settings (with defaults).
        let bot = config.app.as_ref().and_then(|a| a.bot.as_ref());
        let auto_approve = config
            .app
            .as_ref()
            .and_then(|a| a.auto_approve)
            .unwrap_or(false);
        let confirm_timeout = Duration::from_secs(
            bot.and_then(|b| b.confirm_timeout_secs).unwrap_or(60),
        );
        let session_timeout = Duration::from_secs(
            bot.and_then(|b| b.session_timeout_minutes).unwrap_or(30) * 60,
        );
        let global_storage = config
            .app
            .as_ref()
            .and_then(|a| a.global_storage)
            .unwrap_or(false);
        let context_window = llm_client.resolved().context_window;

        // Build the confirmer (optionally with custom keywords).
        let confirmer = match bot.and_then(|b| b.confirm_keywords.as_ref()) {
            Some(kw) => Confirmer::with_keywords(
                Arc::clone(&platform_sender),
                confirm_timeout,
                ConfirmKeywords {
                    approve: kw.approve.clone().unwrap_or_default(),
                    reject: kw.reject.clone().unwrap_or_default(),
                },
            ),
            None => Confirmer::new(Arc::clone(&platform_sender), confirm_timeout),
        };
        let confirmer = Arc::new(confirmer);

        // Open and initialize the database.
        let db_path = resolve_db_path(&working_dir, global_storage)
            .map_err(ManagerError::DbPath)?;
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(&db_path).map_err(ManagerError::DbOpen)?;
        storage::init_db(&conn).map_err(ManagerError::DbInit)?;
        let db = Arc::new(Mutex::new(conn));

        Ok(Self {
            platform,
            agents: Mutex::new(HashMap::new()),
            db,
            config,
            working_dir,
            llm_client,
            tool_registry,
            skill_registry,
            platform_sender,
            confirmer,
            auto_approve,
            context_window,
            session_timeout,
        })
    }

    /// Main event loop. Connects to the platform, then processes events forever.
    pub async fn run(&self) -> Result<(), AgentError> {
        // Spawn the idle-session cleanup loop.
        let cleanup_db = Arc::clone(&self.db);
        let session_timeout = self.session_timeout;
        tokio::spawn(async move {
            cleanup_loop(cleanup_db, session_timeout).await;
        });

        loop {
            match self.platform.recv_event().await {
                Ok(PlatformEvent::Message(msg)) => {
                    self.handle_message(msg).await;
                }
                Ok(PlatformEvent::Disconnected) => {
                    tracing::warn!("Platform disconnected");
                    // MVP: stop. Reconnect logic is a future enhancement.
                    return Ok(());
                }
                Ok(PlatformEvent::Other(v)) => {
                    tracing::debug!("Ignoring platform event: {}", v);
                }
                Err(e) => {
                    tracing::error!("Platform recv error: {}", e);
                    return Err(e);
                }
            }
        }
    }

    /// Process a single incoming chat message.
    async fn handle_message(&self, msg: ChatMessage) {
        let chat_id = msg.sender.chat_id.clone();
        let text = msg.text.trim().to_lowercase();

        // If this is a confirmation reply, route it to the Confirmer (not the Agent).
        if self
            .confirmer
            .check_confirmation_response(&chat_id, &text)
            .is_some()
        {
            return;
        }

        // Normal message → route to (or create) the chat's Agent session.
        match self.get_or_create_session(&chat_id, &msg.text).await {
            Ok(tx) => {
                if let Err(e) = tx.send(FrontendMessage::UserInput(msg.text)).await {
                    tracing::warn!("Failed to send user message to agent for {}: {}", chat_id, e);
                }
            }
            Err(e) => {
                tracing::error!("Failed to get/create session for {}: {}", chat_id, e);
                let _ = self
                    .platform_sender
                    .send(&chat_id, &format!("❌ 内部错误，无法处理消息：{}", e))
                    .await;
            }
        }
    }

    /// Get an existing Agent session for `chat_id`, or create a new one.
    async fn get_or_create_session(
        &self,
        chat_id: &str,
        first_message: &str,
    ) -> Result<mpsc::Sender<FrontendMessage>, AgentError> {
        let mut agents = self.agents.lock().await;
        if let Some(handle) = agents.get_mut(chat_id) {
            handle.last_active_at = Instant::now();
            return Ok(handle.message_tx.clone());
        }
        drop(agents);

        // No active Agent. Check the DB for a persisted session (we still spawn
        // a fresh Agent — history restoration is a future enhancement).
        let session_id = {
            let db = self.db.lock().await;
            match storage::find_session_by_chat_id(&db, chat_id)
                .map_err(|e| AgentError::InternalError(format!("DB lookup failed: {}", e)))?
            {
                Some(info) => info.id,
                None => {
                    // Create a new DB session record.
                    let id = Uuid::new_v4().to_string();
                    let title = generate_title(first_message);
                    let model = self
                        .config
                        .default_model
                        .clone()
                        .unwrap_or_else(|| self.llm_client.model().to_string());
                    storage::insert_session(&db, &id, Some(chat_id), &title, &model, "qq")
                        .map_err(|e| {
                            AgentError::InternalError(format!("DB insert failed: {}", e))
                        })?;
                    id
                }
            }
        };

        let tx = self.spawn_session_agent(chat_id, &session_id).await?;

        let mut agents = self.agents.lock().await;
        agents.insert(
            chat_id.to_string(),
            AgentHandle {
                message_tx: tx.clone(),
                session_id,
                last_active_at: Instant::now(),
            },
        );
        Ok(tx)
    }

    /// Create a `ChatbotFrontend` + `Agent` for a chat and spawn its loop.
    async fn spawn_session_agent(
        &self,
        chat_id: &str,
        session_id: &str,
    ) -> Result<mpsc::Sender<FrontendMessage>, AgentError> {
        let frontend = Arc::new(ChatbotFrontend::new(
            chat_id.to_string(),
            Arc::clone(&self.platform_sender),
            Arc::clone(&self.confirmer),
            self.auto_approve,
        ));

        let (message_tx, message_rx) = mpsc::channel::<FrontendMessage>(16);

        let agent = Agent::new(
            Arc::clone(&self.llm_client),
            Arc::clone(&self.tool_registry),
            Arc::clone(&self.skill_registry),
            frontend,
            self.config.app.as_ref().and_then(|a| a.context.as_ref()),
            self.context_window,
            self.working_dir.clone(),
            self.auto_approve,
        );

        let sid = session_id.to_string();
        let cid = chat_id.to_string();
        tokio::spawn(async move {
            agent.run(message_rx).await;
            tracing::info!("Agent task ended for chat {} (session {})", cid, sid);
        });

        Ok(message_tx)
    }

    /// Number of currently active Agent sessions (for diagnostics / tests).
    pub async fn active_session_count(&self) -> usize {
        self.agents.lock().await.len()
    }
}

/// Errors that can occur while constructing a [`ChatbotManager`].
#[derive(Debug, thiserror::Error)]
pub enum ManagerError {
    #[error("Failed to resolve DB path: {0}")]
    DbPath(String),
    #[error("Failed to open database: {0}")]
    DbOpen(#[from] rusqlite::Error),
    #[error("Failed to initialize database: {0}")]
    DbInit(rusqlite::Error),
}

/// Generate a short session title from the first user message.
fn generate_title(message: &str) -> String {
    let trimmed = message.trim();
    const MAX: usize = 30;
    let chars: Vec<char> = trimmed.chars().take(MAX).collect();
    let mut title: String = chars.into_iter().collect();
    if trimmed.chars().count() > MAX {
        title.push('…');
    }
    if title.is_empty() {
        "QQ 会话".to_string()
    } else {
        title
    }
}

/// Periodically remove idle in-memory Agent sessions.
///
/// The DB session record is preserved (persistence); only the live Agent task
/// is dropped. Dropping the `AgentHandle` drops its `message_tx`, causing the
/// Agent's `run()` loop to exit when it next awaits on the closed channel.
async fn cleanup_loop(_db: Arc<Mutex<Connection>>, _timeout: Duration) {
    // The idle-session cleanup touches the in-memory `agents` map, which lives
    // on the manager. This standalone loop is a placeholder; the manager's
    // `run()` owns the map and could check idle expiry between events. For MVP,
    // sessions live for the process lifetime — acceptable for a single Bot.
    // TODO: wire idle expiry into the run loop or share the agents map here.
    loop {
        tokio::time::sleep(CLEANUP_INTERVAL).await;
        tracing::debug!("cleanup tick (no-op in MVP)");
    }
}

// Keeps ToolCallInfo import referenced for the public surface documentation.
#[allow(dead_code)]
fn _tool_call_info_used(_i: &ToolCallInfo) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{ChatType, SenderInfo};
    use std::collections::VecDeque;

    /// A mock platform that queues events and records sent messages.
    struct MockPlatform {
        events: Mutex<VecDeque<PlatformEvent>>,
        sent: std::sync::Mutex<Vec<(String, String)>>,
    }

    impl MockPlatform {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                events: Mutex::new(VecDeque::new()),
                sent: std::sync::Mutex::new(Vec::new()),
            })
        }

        async fn push_message(&self, chat_id: &str, text: &str) {
            self.events.lock().await.push_back(PlatformEvent::Message(ChatMessage {
                text: text.to_string(),
                sender: SenderInfo {
                    user_id: "u1".into(),
                    chat_id: chat_id.to_string(),
                    chat_type: ChatType::Group,
                },
            }));
        }

        fn sent(&self) -> Vec<(String, String)> {
            self.sent.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl PlatformAdapter for MockPlatform {
        fn capabilities() -> PlatformCaps {
            PlatformCaps::qq()
        }
        async fn send_message(&self, chat_id: &str, text: &str) -> robit_agent::error::Result<SendResult> {
            self.sent
                .lock()
                .unwrap()
                .push((chat_id.to_string(), text.to_string()));
            Ok(SendResult { msg_id: "m1".into() })
        }
        async fn recv_event(&self) -> robit_agent::error::Result<PlatformEvent> {
            // Block-ish: spin until an event is available (test injects events).
            loop {
                if let Some(ev) = self.events.lock().await.pop_front() {
                    return Ok(ev);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }

    #[test]
    fn generate_title_truncates_long_messages() {
        let long = "x".repeat(100);
        let title = generate_title(&long);
        assert!(title.ends_with('…'));
        assert!(title.chars().count() <= 31);
    }

    #[test]
    fn generate_title_short_message() {
        assert_eq!(generate_title("hello"), "hello");
    }

    #[test]
    fn generate_title_empty_message() {
        assert_eq!(generate_title("   "), "QQ 会话");
    }

    // Note: a full end-to-end manager test requires a live LLM client, so it's
    // deferred to manual integration testing. The construction path (new) is
    // exercised via the QQ main entry point in Phase 9.
}
