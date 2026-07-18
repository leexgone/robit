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
use robit_agent::event::{FrontendMessage, SessionId};
use robit_agent::frontend::Frontend;
use robit_agent::storage::{self, resolve_db_path};
use robit_agent::tool::ToolCallInfo;
use robit_agent::{Agent, AgentError, SkillRegistry, ToolRegistry};
use robit_ai::config::RobitConfig;
use robit_ai::LlmClient;
use rusqlite::Connection;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use crate::adapter::{ChatMessage, PlatformAdapter, PlatformCaps, PlatformEvent, SendResult, UploadResult};
use crate::confirmer::{ConfirmKeywords, Confirmer};
use crate::extensions::PlatformExtWrapper;
use crate::frontend::{ChatbotFrontend, PlatformSender, PlatformExt};

/// How often the cleanup loop scans for idle sessions.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(300); // 5 minutes

/// Handle to a running Agent instance for one chat.
pub struct AgentHandle {
    /// Send messages (user input) to the Agent loop.
    pub message_tx: mpsc::Sender<FrontendMessage>,
    pub session_id: String,
    pub last_active_at: Instant,
    /// Frontend reference for saving user messages.
    pub frontend: Arc<ChatbotFrontend>,
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
    async fn upload_file(
        &self,
        chat_id: &str,
        file_path: &str,
        media_type: &str,
    ) -> robit_agent::error::Result<UploadResult> {
        self.platform.upload_file(chat_id, file_path, media_type).await
    }
    async fn send_media_message(
        &self,
        chat_id: &str,
        file_url: &str,
        file_name: &str,
        media_type: &str,
    ) -> robit_agent::error::Result<SendResult> {
        self.platform
            .send_media_message(chat_id, file_url, file_name, media_type)
            .await
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
        let db_path = resolve_db_path(&working_dir, global_storage)?;
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
    /// Returns when the platform disconnects, an error occurs, or `shutdown`
    /// is notified (Ctrl+C in the main binary).
    pub async fn run(
        &self,
        shutdown: Arc<tokio::sync::Notify>,
    ) -> Result<(), AgentError> {
        // Spawn the idle-session cleanup loop (checks shutdown).
        let cleanup_db = Arc::clone(&self.db);
        let session_timeout = self.session_timeout;
        let cleanup_shutdown = shutdown.clone();
        tokio::spawn(async move {
            cleanup_loop(cleanup_db, session_timeout, cleanup_shutdown).await;
        });

        loop {
            tokio::select! {
                event = self.platform.recv_event() => {
                    match event {
                        Ok(PlatformEvent::Message(msg)) => {
                            self.handle_message(msg).await;
                        }
                        Ok(PlatformEvent::Disconnected) => {
                            tracing::warn!("Platform disconnected");
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
                _ = shutdown.notified() => {
                    tracing::info!("Shutdown signal received, stopping event loop...");
                    return Ok(());
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

        // Check for command messages
        let trimmed_text = msg.text.trim();
        if trimmed_text.eq_ignore_ascii_case("/clear") {
            self.handle_clear_command(&chat_id).await;
            return;
        }
        if trimmed_text.eq_ignore_ascii_case("/stop") {
            self.handle_stop_command(&chat_id).await;
            return;
        }
        if trimmed_text.eq_ignore_ascii_case("/new") {
            self.handle_new_command(&chat_id).await;
            return;
        }
        if trimmed_text.eq_ignore_ascii_case("/list") {
            self.handle_list_command(&chat_id).await;
            return;
        }
        if trimmed_text.to_lowercase().starts_with("/switch ") {
            self.handle_switch_command(&chat_id, &trimmed_text["/switch ".len()..]).await;
            return;
        }
        if trimmed_text.eq_ignore_ascii_case("/help") {
            self.handle_help_command(&chat_id).await;
            return;
        }

        // Download and save media files locally
        let media_dir = self.working_dir.join("media");
        for attachment in &msg.attachments {
            if let Err(e) = robit_agent::media::download_media(
                &attachment.url,
                attachment.filename.as_deref(),
                &media_dir,
            )
            .await
            {
                tracing::warn!("Failed to download media: {}", e);
            }
        }

        // Convert attachments to agent's type
        let attachments: Vec<robit_agent::event::MediaAttachment> =
            msg.attachments.into_iter().map(|a| a.into()).collect();

        // Normal message → route to (or create) the chat's Agent session.
        match self.get_or_create_session(&chat_id, &msg.text).await {
            Ok((tx, frontend)) => {
                // Save user message to database
                frontend.save_user_message(&msg.text).await;

                if let Err(e) = tx
                    .send(robit_agent::event::FrontendMessage::UserInput {
                        text: msg.text,
                        attachments,
                    })
                    .await
                {
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

    /// Handle /clear command: clear the current conversation context (in-memory only).
    async fn handle_clear_command(&self, chat_id: &str) {
        // Send "/clear" as a user message to Agent - Agent already has built-in handling for this
        match self.get_or_create_session(chat_id, "clear command").await {
            Ok((tx, _)) => {
                if let Err(e) = tx.send("/clear".into()).await {
                    tracing::warn!("Failed to send /clear to agent: {}", e);
                    let _ = self.platform_sender.send(chat_id, "❌ 清空失败").await;
                }
            }
            Err(e) => {
                tracing::error!("Failed to get session for /clear: {}", e);
                let _ = self.platform_sender.send(chat_id, &format!("❌ 无法执行清空：{}", e)).await;
            }
        }
    }

    /// Handle /stop command: stop the current operation.
    async fn handle_stop_command(&self, chat_id: &str) {
        let agents = self.agents.lock().await;
        if let Some(handle) = agents.get(chat_id) {
            // Send Cancel message to Agent
            if let Err(e) = handle.message_tx.send(robit_agent::event::FrontendMessage::Cancel).await {
                tracing::warn!("Failed to send Cancel to agent: {}", e);
                let _ = self.platform_sender.send(chat_id, "❌ 停止失败").await;
                return;
            }
            let _ = self.platform_sender.send(chat_id, "⏹️ 已发送停止信号").await;
        } else {
            let _ = self.platform_sender.send(chat_id, "ℹ️ 当前没有活动的会话").await;
        }
    }

    /// Handle /new command: create a fresh conversation session.
    async fn handle_new_command(&self, chat_id: &str) {
        let mut agents = self.agents.lock().await;

        // 1. 如果有当前会话，先关闭它
        if let Some(old_handle) = agents.remove(chat_id) {
            // 把旧会话标记为不活跃
            let db = self.db.lock().await;
            if let Err(e) = robit_agent::storage::delete_session(&db, &old_handle.session_id) {
                tracing::warn!("Failed to deactivate old session: {}", e);
            }
            drop(db);

            // 丢弃旧的 Agent 通道，让任务自然结束
            drop(old_handle);
        }
        drop(agents);

        // 2. 创建新会话（会自动触发 get_or_create_session）
        match self.get_or_create_session(chat_id, "新会话").await {
            Ok((_, frontend)) => {
                let msg = format!(
                    "✨ 已创建新会话\n会话ID: {}\n旧会话已归档，使用 /list 查看历史",
                    frontend.session_id
                );
                let _ = self.platform_sender.send(chat_id, &msg).await;
            }
            Err(e) => {
                tracing::error!("Failed to create new session: {}", e);
                let _ = self.platform_sender.send(chat_id, &format!("❌ 创建新会话失败：{}", e)).await;
            }
        }
    }

    /// Handle /list command: list all sessions for this chat.
    async fn handle_list_command(&self, chat_id: &str) {
        let db = self.db.lock().await;
        match robit_agent::storage::list_all_sessions_by_chat_id(&db, chat_id) {
            Ok(sessions) if sessions.is_empty() => {
                let _ = self.platform_sender.send(chat_id, "ℹ️ 暂无历史会话").await;
            }
            Ok(sessions) => {
                let mut list_text = String::from("📜 历史会话列表\n\n");
                for (i, session) in sessions.iter().enumerate() {
                    let indicator = if i == 0 { "👉" } else { "  " };
                    let current_mark = if i == 0 { " [当前]" } else { "" };
                    list_text.push_str(&format!(
                        "{} {}. {}{}\n",
                        indicator,
                        i + 1,
                        session.title,
                        current_mark
                    ));
                    list_text.push_str(&format!(
                        "   ID: {} | 创建: {}\n\n",
                        session.id,
                        session.created_at
                    ));
                }
                list_text.push_str("💡 使用 /switch <序号> 切换到对应会话");
                let _ = self.platform_sender.send(chat_id, &list_text).await;
            }
            Err(e) => {
                tracing::error!("Failed to list sessions: {}", e);
                let _ = self.platform_sender.send(chat_id, &format!("❌ 获取会话列表失败：{}", e)).await;
            }
        }
    }

    /// Handle /switch command: switch to a specific session.
    async fn handle_switch_command(&self, chat_id: &str, arg: &str) {
        let trimmed_arg = arg.trim();

        // 解析序号（支持数字）
        let session_index = match trimmed_arg.parse::<usize>() {
            Ok(n) if n > 0 => n - 1, // 转换为0-based索引
            _ => {
                let _ = self.platform_sender.send(chat_id, "❌ 请输入有效的会话序号，如 /switch 1").await;
                return;
            }
        };

        // 获取会话列表
        let db = self.db.lock().await;
        let sessions = match robit_agent::storage::list_all_sessions_by_chat_id(&db, chat_id) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to list sessions: {}", e);
                let _ = self.platform_sender.send(chat_id, &format!("❌ 获取会话列表失败：{}", e)).await;
                return;
            }
        };
        drop(db);

        // 检查序号是否有效
        if session_index >= sessions.len() {
            let _ = self.platform_sender.send(
                chat_id,
                &format!("❌ 会话序号无效，共有 {} 个会话", sessions.len())
            ).await;
            return;
        }

        let target_session = &sessions[session_index];
        let target_id = target_session.id.clone();

        // 如果已经是当前会话，不需要切换
        {
            let agents = self.agents.lock().await;
            if let Some(current) = agents.get(chat_id) {
                if current.session_id == target_id {
                    let _ = self.platform_sender.send(chat_id, "ℹ️ 已经是当前会话").await;
                    return;
                }
            }
        }

        // 在数据库中激活目标会话，停用其他会话
        let db = self.db.lock().await;
        if let Err(e) = robit_agent::storage::activate_session(&db, &target_id, chat_id) {
            tracing::error!("Failed to activate session: {}", e);
            let _ = self.platform_sender.send(chat_id, &format!("❌ 切换失败：{}", e)).await;
            return;
        }
        drop(db);

        // 替换内存中的 Agent
        let mut agents = self.agents.lock().await;
        // 先移除旧的
        agents.remove(chat_id);
        drop(agents);

        // 创建新的 Agent
        match self.get_or_create_session(chat_id, "切换会话").await {
            Ok((_, _frontend)) => {
                let _ = self.platform_sender.send(
                    chat_id,
                    &format!("✅ 已切换到会话：{}", target_session.title)
                ).await;
            }
            Err(e) => {
                tracing::error!("Failed to create agent after switch: {}", e);
                let _ = self.platform_sender.send(chat_id, &format!("❌ 会话加载失败：{}", e)).await;
            }
        }
    }

    /// Handle /help command: show available commands.
    async fn handle_help_command(&self, chat_id: &str) {
        let help_text = r#"🤖 Robit 帮助

可用指令：
- /clear - 清空当前对话上下文（仅内存中）
- /stop - 停止当前执行
- /new - 创建新会话（旧会话归档）
- /list - 列出所有历史会话
- /switch <序号> - 切换到指定会话
- /help - 显示此帮助

提示：直接发送消息与机器人对话即可。"#;
        let _ = self.platform_sender.send(chat_id, help_text).await;
    }

    /// Get an existing Agent session for `chat_id`, or create a new one.
    async fn get_or_create_session(
        &self,
        chat_id: &str,
        first_message: &str,
    ) -> Result<(mpsc::Sender<FrontendMessage>, Arc<ChatbotFrontend>), AgentError> {
        let mut agents = self.agents.lock().await;
        if let Some(handle) = agents.get_mut(chat_id) {
            handle.last_active_at = Instant::now();
            tracing::debug!("get_or_create_session: found active agent in memory for chat_id={}, session_id={}", chat_id, handle.session_id);
            return Ok((handle.message_tx.clone(), handle.frontend.clone()));
        }
        drop(agents);

        // No active Agent. Check the DB for a persisted session.
        let session_id = {
            let db = self.db.lock().await;
            match storage::find_session_by_chat_id(&db, chat_id)
                .map_err(|e| AgentError::InternalError(format!("DB lookup failed: {}", e)))?
            {
                Some(info) => {
                    tracing::info!("get_or_create_session: found existing session in DB for chat_id={}, session_id={}, title={}", chat_id, info.id, info.title);
                    info.id
                }
                None => {
                    // Create a new DB session record.
                    let id = Uuid::new_v4().to_string();
                    let title = generate_title(first_message);
                    let model = self
                        .config
                        .default_model
                        .clone()
                        .unwrap_or_else(|| self.llm_client.model().to_string());
                    tracing::info!("get_or_create_session: creating new session in DB for chat_id={}, session_id={}, title={}", chat_id, id, title);
                    storage::insert_session(&db, &id, Some(chat_id), &title, &model, "qq")
                        .map_err(|e| {
                            AgentError::InternalError(format!("DB insert failed: {}", e))
                        })?;
                    id
                }
            }
        };

        let (tx, frontend) = self.spawn_session_agent(chat_id, &session_id).await?;

        let mut agents = self.agents.lock().await;
        agents.insert(
            chat_id.to_string(),
            AgentHandle {
                message_tx: tx.clone(),
                session_id,
                last_active_at: Instant::now(),
                frontend: frontend.clone(),
            },
        );
        Ok((tx, frontend))
    }

    /// Create a `ChatbotFrontend` + `Agent` for a chat and spawn its loop.
    async fn spawn_session_agent(
        &self,
        chat_id: &str,
        session_id: &str,
    ) -> Result<(mpsc::Sender<FrontendMessage>, Arc<ChatbotFrontend>), AgentError> {
        tracing::info!("spawn_session_agent: chat_id={}, session_id={}", chat_id, session_id);

        let frontend = Arc::new(ChatbotFrontend::new(
            chat_id.to_string(),
            session_id.to_string(),
            Arc::clone(&self.platform_sender),
            Arc::clone(&self.confirmer),
            Arc::clone(&self.db),
            self.auto_approve,
        ));

        let (message_tx, message_rx) = mpsc::channel::<FrontendMessage>(16);

        // Load historical messages from database
        tracing::debug!("spawn_session_agent: loading history messages from DB...");
        let db = self.db.lock().await;
        let history_messages = robit_agent::storage::load_chat_messages(&db, session_id)
            .unwrap_or_default();
        drop(db);

        tracing::info!("spawn_session_agent: loaded {} history messages", history_messages.len());

        // Parse session_id string to SessionId
        let session_id_obj = SessionId::from(session_id.to_string());

        tracing::debug!("spawn_session_agent: creating Agent with history...");
        let agent = Agent::with_history(
            Arc::clone(&self.llm_client),
            Arc::clone(&self.tool_registry),
            Arc::clone(&self.skill_registry),
            Arc::clone(&frontend) as Arc<dyn Frontend>,
            self.config.app.as_ref().and_then(|a| a.context.as_ref()),
            self.context_window,
            self.working_dir.clone(),
            self.auto_approve,
            {
                let mut exts = HashMap::new();
                let platform_ext: Arc<dyn PlatformExt> = frontend.clone();
                exts.insert(
                    crate::extensions::keys::PLATFORM_EXT.to_string(),
                    PlatformExtWrapper::new(platform_ext),
                );
                exts
            },
            session_id_obj,
            history_messages,
        );

        let sid = session_id.to_string();
        let cid = chat_id.to_string();
        tokio::spawn(async move {
            agent.run(message_rx).await;
            tracing::info!("Agent task ended for chat {} (session {})", cid, sid);
        });

        Ok((message_tx, frontend))
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
    DbPath(#[from] robit_agent::AgentError),
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
async fn cleanup_loop(
    _db: Arc<Mutex<Connection>>,
    _timeout: Duration,
    shutdown: Arc<tokio::sync::Notify>,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(CLEANUP_INTERVAL) => {
                tracing::debug!("cleanup tick (no-op in MVP)");
            }
            _ = shutdown.notified() => {
                tracing::debug!("cleanup loop received shutdown signal");
                return;
            }
        }
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
    #[allow(dead_code)]
    struct MockPlatform {
        events: Mutex<VecDeque<PlatformEvent>>,
        sent: std::sync::Mutex<Vec<(String, String)>>,
    }

    #[allow(dead_code)]
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
                attachments: vec![],
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
