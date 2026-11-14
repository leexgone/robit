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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use robit_agent::event::{FrontendMessage, SessionId};
use robit_agent::frontend::Frontend;
use robit_agent::storage::{self, resolve_db_path};
use robit_agent::tool::ToolCallInfo;
use robit_agent::{Agent, AgentError, SkillRegistry, ToolRegistry};
use robit_ai::config::{BotConfig, MediaRetentionConfig, RobitConfig};
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
    /// True while a turn is in flight (shared with the frontend). The idle
    /// cleanup skips busy sessions so long turns are never evicted mid-run.
    pub busy: Arc<AtomicBool>,
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
    /// Active Agent instances, keyed by chat_id. Shared with the cleanup loop.
    agents: Arc<Mutex<HashMap<String, AgentHandle>>>,
    /// Platform identifier recorded as session `source` (from the adapter).
    source: &'static str,
    /// Instance namespace for session identity (`[app.bot] instance_id`,
    /// default `""`); separates same-platform instances sharing one DB (H4).
    namespace: String,
    /// Host-provided persona prompt (config `[app.bot] system_prompt` /
    /// `system_prompt_file`); `None` keeps the `agent.md` file lookup.
    custom_prompt: Option<String>,
    /// Retention policy for the inbound `media/` directory (default off).
    media_retention: MediaRetentionConfig,
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
        let custom_prompt = resolve_custom_prompt(bot, &working_dir);
        let media_retention = bot.and_then(|b| b.media.clone()).unwrap_or_default();
        let namespace = bot
            .and_then(|b| b.instance_id.clone())
            .unwrap_or_default();

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
            agents: Arc::new(Mutex::new(HashMap::new())),
            source: T::platform_name(),
            namespace,
            custom_prompt,
            media_retention,
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
        let cleanup_agents = Arc::clone(&self.agents);
        let session_timeout = self.session_timeout;
        let media_dir = self.working_dir.join("media");
        let media_retention = self.media_retention.clone();
        let cleanup_shutdown = shutdown.clone();
        tokio::spawn(async move {
            cleanup_loop(
                cleanup_agents,
                session_timeout,
                media_dir,
                media_retention,
                cleanup_shutdown,
            )
            .await;
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
        if trimmed_text.eq_ignore_ascii_case("/cancel")
            || trimmed_text.to_lowercase().starts_with("/cancel ")
        {
            let arg = trimmed_text
                .strip_prefix("/cancel")
                .unwrap_or("")
                .trim();
            self.handle_cancel_command(&chat_id, arg).await;
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

        // Download and save media files locally, backfilling the saved path
        // into each attachment: the agent encodes images from the local copy
        // (CDN URLs expire), and host-side tools can consume the file.
        let media_dir = self.working_dir.join("media");
        let max_media_bytes = self
            .config
            .app
            .as_ref()
            .and_then(|a| a.context.as_ref())
            .and_then(|c| c.max_media_bytes)
            .unwrap_or(robit_agent::context::DEFAULT_MAX_MEDIA_BYTES);
        let mut attachments: Vec<robit_agent::event::MediaAttachment> =
            Vec::with_capacity(msg.attachments.len());
        for att in msg.attachments {
            let opts = robit_agent::media::MediaDownloadOptions {
                max_bytes: max_media_bytes,
                content_type: Some(att.content_type.clone()),
            };
            let local_path = match robit_agent::media::download_media_with_opts(
                &att.url,
                att.filename.as_deref(),
                &media_dir,
                &opts,
            )
            .await
            {
                Ok(path) => Some(path),
                Err(e) => {
                    tracing::warn!("Failed to download media {}: {}", att.url, e);
                    None
                }
            };
            let mut converted: robit_agent::event::MediaAttachment = att.into();
            converted.local_path = local_path;
            attachments.push(converted);
        }

        // Archive/title text carries one annotation line per attachment;
        // model-bound text stays raw (the agent annotates it exactly once).
        let archive_text = annotated_text(&msg.text, &attachments);

        // Normal message → route to (or create) the chat's Agent session.
        // The archive row is written only AFTER the agent accepted the input,
        // so a dead handle can never leave "archived but never seen" messages.
        match self.route_user_input(&chat_id, &archive_text, msg.text, attachments).await {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("Failed to get/create session for {}: {}", chat_id, e);
                let _ = self
                    .platform_sender
                    .send(&chat_id, &format!("❌ 内部错误，无法处理消息：{}", e))
                    .await;
            }
        }
    }

    /// Deliver a user turn to the chat's Agent, self-healing dead handles.
    ///
    /// On send failure (e.g. the Agent task exited after `/exit` while the
    /// handle lingered in the map) the stale handle is evicted and the session
    /// is recreated from the DB once. The archive row is written only after a
    /// successful send. Returns the last error if the retry also fails.
    async fn route_user_input(
        &self,
        chat_id: &str,
        archive_text: &str,
        text: String,
        attachments: Vec<robit_agent::event::MediaAttachment>,
    ) -> Result<(), AgentError> {
        let (tx, frontend) = self.get_or_create_session(chat_id, archive_text).await?;
        let input = FrontendMessage::UserInput {
            text: text.clone(),
            attachments: attachments.clone(),
        };
        if tx.send(input).await.is_ok() {
            frontend.save_user_message(archive_text).await;
            return Ok(());
        }

        // Dead channel: evict the stale handle and respawn from the DB.
        tracing::warn!(
            "Agent channel dead for {}; evicting handle and recreating session",
            chat_id
        );
        self.agents.lock().await.remove(chat_id);
        let (tx, frontend) = self.get_or_create_session(chat_id, archive_text).await?;
        let input = FrontendMessage::UserInput { text, attachments };
        tx.send(input).await.map_err(|e| {
            AgentError::InternalError(format!("Failed to deliver message after respawn: {}", e))
        })?;
        frontend.save_user_message(archive_text).await;
        Ok(())
    }

    /// Inject a user turn into a chat's session from the host side
    /// (scheduler, automation, etc.) — the public alternative to synthesizing
    /// fake inbound platform events (H1).
    ///
    /// Unlike real messages this bypasses confirmation-keyword routing and
    /// slash-command parsing: `text` reaches the model verbatim. The archive
    /// row is marked with `source` (e.g. `"scheduler"`) so injected turns are
    /// distinguishable from real user messages; injected content remains part
    /// of the session history the model sees on later turns. Tool execution
    /// still goes through the normal confirmation flow.
    ///
    /// Fails with an error when the session cannot be created or the input
    /// cannot be delivered (after one dead-handle respawn); the caller decides
    /// whether/how to notify anyone.
    pub async fn inject_message(
        &self,
        chat_id: &str,
        text: &str,
        source: &str,
    ) -> Result<(), AgentError> {
        if source.trim().is_empty() {
            return Err(AgentError::InternalError(
                "inject_message: source must be a non-empty label".to_string(),
            ));
        }
        let (tx, frontend) = self.get_or_create_session(chat_id, text).await?;
        let input = FrontendMessage::UserInput {
            text: text.to_string(),
            attachments: vec![],
        };
        let frontend = if tx.send(input).await.is_ok() {
            frontend
        } else {
            tracing::warn!(
                "Agent channel dead for {} during injection; recreating session",
                chat_id
            );
            self.agents.lock().await.remove(chat_id);
            let (tx, frontend) = self.get_or_create_session(chat_id, text).await?;
            tx.send(FrontendMessage::UserInput {
                text: text.to_string(),
                attachments: vec![],
            })
            .await
            .map_err(|e| {
                AgentError::InternalError(format!("Failed to inject after respawn: {}", e))
            })?;
            frontend
        };
        frontend
            .save_user_message_with_source(text, Some(source))
            .await;
        Ok(())
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

    /// Handle /cancel command: cancel async background task(s).
    ///
    /// `/cancel` with no argument cancels all pending tasks; `/cancel <task_id>`
    /// cancels a specific one.
    async fn handle_cancel_command(&self, chat_id: &str, arg: &str) {
        let agents = self.agents.lock().await;
        if let Some(handle) = agents.get(chat_id) {
            let msg = if arg.is_empty() {
                let _ = handle
                    .message_tx
                    .send(robit_agent::event::FrontendMessage::Cancel)
                    .await;
                "⏹️ 已发送取消全部后台任务的信号".to_string()
            } else {
                let _ = handle
                    .message_tx
                    .send(robit_agent::event::FrontendMessage::CancelTask {
                        task_id: arg.to_string(),
                    })
                    .await;
                format!("⏹️ 已发送取消任务 {} 的信号", arg)
            };
            let _ = self.platform_sender.send(chat_id, &msg).await;
        } else {
            let _ = self.platform_sender
                .send(chat_id, "ℹ️ 当前没有活动的会话")
                .await;
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
        match robit_agent::storage::list_all_sessions_by_chat_id(&db, chat_id, self.source, &self.namespace) {
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
        let sessions = match robit_agent::storage::list_all_sessions_by_chat_id(&db, chat_id, self.source, &self.namespace) {
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
        if let Err(e) = robit_agent::storage::activate_session(&db, &target_id, chat_id, self.source, &self.namespace) {
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
- /cancel [task_id] - 取消后台任务（无参数取消全部）
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
            match storage::find_session_by_chat_id(&db, chat_id, self.source, &self.namespace)
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
                    storage::insert_session(&db, &id, Some(chat_id), &title, &model, self.source, &self.namespace)
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
                busy: frontend.busy.clone(),
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
        let memory = robit_agent::memory::resolve_memory_settings(&self.config, &self.working_dir)?;
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
            memory,
            self.custom_prompt.clone(),
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

/// Build archive/title text: raw message text plus one annotation line per
/// attachment, using the same `describe` implementation the agent applies to
/// model-bound text. Keeps attachment-only messages from archiving as empty.
fn annotated_text(text: &str, attachments: &[robit_agent::event::MediaAttachment]) -> String {
    if attachments.is_empty() {
        return text.to_string();
    }
    let descs: Vec<String> = attachments.iter().map(|a| a.describe()).collect();
    if text.is_empty() {
        descs.join("\n")
    } else {
        format!("{}\n{}", text, descs.join("\n"))
    }
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

/// Periodically remove idle/dead in-memory Agent sessions and rotate the
/// inbound media directory (H3 + M7).
///
/// The DB session record is preserved (persistence); only the live Agent task
/// is dropped. Dropping the `AgentHandle` drops its `message_tx`, causing the
/// Agent's `run()` loop to exit when it next awaits on the closed channel.
/// A chat that messages again afterwards gets a fresh Agent backed by the
/// same DB record (`get_or_create_session`).
///
/// Busy sessions (turn in flight, including tool-confirmation waits) are never
/// evicted. Dead handles (Agent task exited, e.g. after `/exit`) are removed
/// regardless of age so the next message respawns instead of hitting a closed
/// channel.
async fn cleanup_loop(
    agents: Arc<Mutex<HashMap<String, AgentHandle>>>,
    timeout: Duration,
    media_dir: PathBuf,
    media_retention: MediaRetentionConfig,
    shutdown: Arc<tokio::sync::Notify>,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(CLEANUP_INTERVAL) => {
                cleanup_tick(&agents, timeout, &media_dir, &media_retention).await;
            }
            _ = shutdown.notified() => {
                tracing::debug!("cleanup loop received shutdown signal");
                return;
            }
        }
    }
}

/// One cleanup pass: evict idle sessions, remove dead handles, rotate media.
async fn cleanup_tick(
    agents: &Arc<Mutex<HashMap<String, AgentHandle>>>,
    timeout: Duration,
    media_dir: &std::path::Path,
    media_retention: &MediaRetentionConfig,
) {
    let mut idle_evicted = 0usize;
    let mut dead_removed = 0usize;
    {
        let mut map = agents.lock().await;
        map.retain(|chat_id, handle| {
            if handle.message_tx.is_closed() {
                tracing::info!(
                    "cleanup: removing dead agent handle for chat {} (session {})",
                    chat_id, handle.session_id
                );
                dead_removed += 1;
                return false;
            }
            if !handle.busy.load(Ordering::Relaxed)
                && handle.last_active_at.elapsed() > timeout
            {
                tracing::info!(
                    "cleanup: evicting idle session for chat {} (session {}, idle {:?})",
                    chat_id, handle.session_id, handle.last_active_at.elapsed()
                );
                idle_evicted += 1;
                return false;
            }
            true
        });
    }
    if idle_evicted > 0 || dead_removed > 0 {
        tracing::debug!(
            "cleanup tick: {} idle evicted, {} dead removed",
            idle_evicted, dead_removed
        );
    }
    rotate_media(media_dir, media_retention).await;
}

/// Apply the media retention policy (M7). Both knobs default to 0 = off.
/// Only top-level files in `media_dir` are considered (no recursion).
async fn rotate_media(media_dir: &std::path::Path, retention: &MediaRetentionConfig) {
    let retention_days = retention.retention_days.unwrap_or(0);
    let max_files = retention.max_files.unwrap_or(0);
    if retention_days == 0 && max_files == 0 {
        return;
    }

    let mut entries: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    let mut read_dir = match tokio::fs::read_dir(media_dir).await {
        Ok(rd) => rd,
        Err(_) => return, // directory not created yet — nothing to rotate
    };
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let path = entry.path();
        let meta = match entry.metadata().await {
            Ok(m) if m.is_file() => m,
            _ => continue,
        };
        if let Ok(mtime) = meta.modified() {
            entries.push((path, mtime));
        }
    }

    let mut removed = 0usize;
    if retention_days > 0 {
        let cutoff = std::time::SystemTime::now()
            - Duration::from_secs(retention_days * 24 * 3600);
        let (old, fresh): (Vec<_>, Vec<_>) = entries.into_iter().partition(|(_, mtime)| *mtime < cutoff);
        for (path, _) in old {
            if tokio::fs::remove_file(&path).await.is_ok() {
                removed += 1;
            }
        }
        entries = fresh;
    }
    if max_files > 0 && entries.len() > max_files {
        // Newest first; drop the tail beyond the count cap.
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        for (path, _) in entries.iter().skip(max_files) {
            if tokio::fs::remove_file(path).await.is_ok() {
                removed += 1;
            }
        }
    }
    if removed > 0 {
        tracing::info!("media rotation: removed {} file(s) from {}", removed, media_dir.display());
    }
}

/// Resolve the host-provided persona prompt (H6):
/// `system_prompt` string > `system_prompt_file` contents > `None`
/// (the Agent then falls back to the `agent.md` file lookup, unchanged).
fn resolve_custom_prompt(bot: Option<&BotConfig>, working_dir: &std::path::Path) -> Option<String> {
    let bot = bot?;
    if let Some(prompt) = &bot.system_prompt {
        return Some(prompt.clone());
    }
    if let Some(file) = &bot.system_prompt_file {
        // `join` keeps absolute paths as-is and resolves relative ones
        // against the working directory.
        let path = working_dir.join(file);
        match std::fs::read_to_string(&path) {
            Ok(content) => return Some(content),
            Err(e) => tracing::warn!(
                "Failed to read system_prompt_file {}: {}; falling back to agent.md lookup",
                path.display(),
                e
            ),
        }
    }
    None
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
        fn platform_name() -> &'static str {
            "mock"
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

    fn att(content_type: &str, filename: Option<&str>) -> robit_agent::event::MediaAttachment {
        robit_agent::event::MediaAttachment {
            content_type: content_type.to_string(),
            url: "http://example.invalid/f".to_string(),
            filename: filename.map(|s| s.to_string()),
            size: Some(1024),
            width: None,
            height: None,
            local_path: None,
        }
    }

    #[test]
    fn annotated_text_appends_one_line_per_attachment() {
        let atts = vec![att("text/csv", Some("a.csv")), att("image/png", Some("b.png"))];
        let out = annotated_text("看看", &atts);
        assert!(out.starts_with("看看\n"));
        assert_eq!(out.matches("[用户发送了").count(), 2);
    }

    #[test]
    fn annotated_text_attachment_only_message_is_not_empty() {
        let atts = vec![att("text/csv", Some("对账单.csv"))];
        let out = annotated_text("", &atts);
        assert_eq!(out, "[用户发送了文件: 对账单.csv (1.0KB)]");
        // Title derived from it must not degrade to the empty fallback.
        assert_ne!(generate_title(&out), "QQ 会话");
    }

    #[test]
    fn annotated_text_without_attachments_is_unchanged() {
        assert_eq!(annotated_text("普通消息", &[]), "普通消息");
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

    // ========================================================================
    // Manager-level tests (offline LLM client: construction and routing only)
    // ========================================================================

    use robit_agent::{SkillRegistry, ToolRegistry};
    use robit_ai::LlmClient;

    fn offline_config() -> RobitConfig {
        toml::from_str(
            r#"
            default_model = "test/text"
            [providers.test]
            base_url = "http://127.0.0.1:1"
            api_key = "sk-test"
            [[providers.test.models]]
            id = "text"
            "#,
        )
        .unwrap()
    }

    fn make_manager(
        working_dir: PathBuf,
        config: RobitConfig,
    ) -> ChatbotManager<MockPlatform> {
        let llm = Arc::new(LlmClient::from_config(&config, None).unwrap());
        ChatbotManager::new(
            MockPlatform::new(),
            config,
            working_dir,
            llm,
            Arc::new(ToolRegistry::new()),
            Arc::new(SkillRegistry::new(vec![], &[])),
        )
        .unwrap()
    }

    fn make_handle(manager: &ChatbotManager<MockPlatform>, session_id: &str) -> AgentHandle {
        let (tx, _rx) = mpsc::channel::<FrontendMessage>(16);
        // Keep the receiver alive so the channel stays open.
        std::mem::forget(_rx);
        let frontend = Arc::new(ChatbotFrontend::new(
            "chat".to_string(),
            session_id.to_string(),
            Arc::clone(&manager.platform_sender),
            Arc::clone(&manager.confirmer),
            Arc::clone(&manager.db),
            false,
        ));
        AgentHandle {
            message_tx: tx,
            session_id: session_id.to_string(),
            last_active_at: Instant::now(),
            busy: frontend.busy.clone(),
            frontend,
        }
    }

    #[tokio::test]
    async fn inject_message_bypasses_slash_parsing_and_marks_source() {
        let dir = tempfile::tempdir().unwrap();
        let manager = make_manager(dir.path().to_path_buf(), offline_config());

        manager
            .inject_message("group:1", "/help", "scheduler")
            .await
            .unwrap();

        // Archive row carries the injection marker and the verbatim text.
        let db = manager.db.lock().await;
        let session = storage::find_session_by_chat_id(&db, "group:1", "mock", "")
            .unwrap()
            .expect("session created");
        // H2: source comes from the adapter, not a hardcoded "qq".
        assert_eq!(session.source, "mock");
        let messages = storage::get_messages(&db, &session.id).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "/help");
        assert_eq!(messages[0].source.as_deref(), Some("scheduler"));
        drop(db);

        // Slash parsing must NOT have run: no help text was sent. (The agent
        // turn itself fails against the offline LLM — that reply is an error
        // notice, never the /help command output.)
        let sent = manager.platform.sent();
        assert!(
            !sent.iter().any(|(_, text)| text.contains("可用指令")),
            "injected /help must reach the model verbatim, got {:?}",
            sent
        );
    }

    #[tokio::test]
    async fn inject_message_rejects_empty_source() {
        let dir = tempfile::tempdir().unwrap();
        let manager = make_manager(dir.path().to_path_buf(), offline_config());
        assert!(manager.inject_message("group:1", "hi", "  ").await.is_err());
    }

    #[tokio::test]
    async fn route_user_input_self_heals_dead_handle() {
        let dir = tempfile::tempdir().unwrap();
        let manager = make_manager(dir.path().to_path_buf(), offline_config());

        // Plant a dead handle: receiver dropped → channel closed.
        let (tx, rx) = mpsc::channel::<FrontendMessage>(16);
        drop(rx);
        let frontend = Arc::new(ChatbotFrontend::new(
            "group:2".to_string(),
            "dead-session".to_string(),
            Arc::clone(&manager.platform_sender),
            Arc::clone(&manager.confirmer),
            Arc::clone(&manager.db),
            false,
        ));
        manager.agents.lock().await.insert(
            "group:2".to_string(),
            AgentHandle {
                message_tx: tx,
                session_id: "dead-session".to_string(),
                last_active_at: Instant::now(),
                busy: frontend.busy.clone(),
                frontend,
            },
        );

        manager
            .route_user_input("group:2", "你好", "你好".to_string(), vec![])
            .await
            .unwrap();

        // Handle was replaced with a live one; archive row written once.
        let agents = manager.agents.lock().await;
        let handle = agents.get("group:2").unwrap();
        assert_ne!(handle.session_id, "dead-session");
        assert!(!handle.message_tx.is_closed());
        drop(agents);
        let db = manager.db.lock().await;
        let session = storage::find_session_by_chat_id(&db, "group:2", "mock", "").unwrap().unwrap();
        let messages = storage::get_messages(&db, &session.id).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "你好");
        assert!(messages[0].source.is_none(), "real message must have no marker");
    }

    #[tokio::test]
    async fn cleanup_tick_evicts_idle_and_dead_but_spares_busy() {
        let dir = tempfile::tempdir().unwrap();
        let manager = make_manager(dir.path().to_path_buf(), offline_config());
        let timeout = Duration::from_secs(60);

        let mut idle = make_handle(&manager, "s-idle");
        idle.last_active_at = Instant::now() - Duration::from_secs(120);
        let fresh = make_handle(&manager, "s-fresh");
        let mut busy = make_handle(&manager, "s-busy");
        busy.last_active_at = Instant::now() - Duration::from_secs(120);
        busy.busy.store(true, Ordering::Relaxed);
        let mut dead = make_handle(&manager, "s-dead");
        // Close the channel by dropping the sender's receiver counterpart:
        // make_handle forgot the rx, so simulate death via a fresh closed pair.
        let (tx, rx) = mpsc::channel::<FrontendMessage>(16);
        drop(rx);
        dead.message_tx = tx;

        {
            let mut map = manager.agents.lock().await;
            map.insert("idle".into(), idle);
            map.insert("fresh".into(), fresh);
            map.insert("busy".into(), busy);
            map.insert("dead".into(), dead);
        }

        cleanup_tick(
            &manager.agents,
            timeout,
            &dir.path().join("media"),
            &MediaRetentionConfig::default(),
        )
        .await;

        let map = manager.agents.lock().await;
        assert!(!map.contains_key("idle"), "idle session must be evicted");
        assert!(!map.contains_key("dead"), "dead handle must be removed");
        assert!(map.contains_key("fresh"), "recent session must stay");
        assert!(map.contains_key("busy"), "busy session must never be evicted");
    }

    #[tokio::test]
    async fn rotate_media_applies_age_and_count_caps() {
        use std::time::SystemTime;
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        tokio::fs::create_dir_all(&media).await.unwrap();

        let now = SystemTime::now();
        let old = now - Duration::from_secs(10 * 24 * 3600);
        for (name, mtime) in [
            ("old1.bin", old),
            ("old2.bin", old),
            ("new1.bin", now - Duration::from_secs(3600)),
            ("new2.bin", now - Duration::from_secs(1800)),
            ("new3.bin", now),
        ] {
            let path = media.join(name);
            tokio::fs::write(&path, b"x").await.unwrap();
            let f = std::fs::File::options().write(true).open(&path).unwrap();
            f.set_modified(mtime).unwrap();
        }

        // Age cap removes the two 10-day-old files.
        let retention = MediaRetentionConfig {
            retention_days: Some(7),
            max_files: None,
        };
        rotate_media(&media, &retention).await;
        assert!(!media.join("old1.bin").exists());
        assert!(!media.join("old2.bin").exists());
        assert!(media.join("new3.bin").exists());

        // Count cap keeps only the newest 2 of the remaining 3.
        let retention = MediaRetentionConfig {
            retention_days: None,
            max_files: Some(2),
        };
        rotate_media(&media, &retention).await;
        assert!(!media.join("new1.bin").exists(), "oldest survivor must go");
        assert!(media.join("new2.bin").exists());
        assert!(media.join("new3.bin").exists());

        // Default (both off) is a no-op.
        rotate_media(&media, &MediaRetentionConfig::default()).await;
        assert!(media.join("new2.bin").exists());
    }

    #[test]
    fn resolve_custom_prompt_priority_string_over_file_over_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("persona.md"), "FROM FILE").unwrap();

        let bot: BotConfig = toml::from_str(
            r#"
            system_prompt = "FROM STRING"
            system_prompt_file = "persona.md"
            "#,
        )
        .unwrap();
        assert_eq!(
            resolve_custom_prompt(Some(&bot), dir.path()).as_deref(),
            Some("FROM STRING")
        );

        let bot: BotConfig = toml::from_str(r#"system_prompt_file = "persona.md""#).unwrap();
        assert_eq!(
            resolve_custom_prompt(Some(&bot), dir.path()).as_deref(),
            Some("FROM FILE")
        );

        let bot: BotConfig = toml::from_str(r#"system_prompt_file = "missing.md""#).unwrap();
        assert_eq!(resolve_custom_prompt(Some(&bot), dir.path()), None);

        assert_eq!(resolve_custom_prompt(None, dir.path()), None);
    }

    fn config_with_instance(instance_id: &str) -> RobitConfig {
        toml::from_str(&format!(
            r#"
            default_model = "test/text"
            [providers.test]
            base_url = "http://127.0.0.1:1"
            api_key = "sk-test"
            [[providers.test.models]]
            id = "text"
            [app.bot]
            instance_id = "{instance_id}"
            "#
        ))
        .unwrap()
    }

    /// H4: two same-platform instances sharing one working dir must not
    /// hijack each other's sessions on chat_id collision.
    #[tokio::test]
    async fn two_instances_share_db_without_hijacking_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let manager_a = make_manager(dir.path().to_path_buf(), config_with_instance("inst-a"));
        let manager_b = make_manager(dir.path().to_path_buf(), config_with_instance("inst-b"));

        manager_a.inject_message("group:dup", "from A", "scheduler").await.unwrap();
        manager_b.inject_message("group:dup", "from B", "scheduler").await.unwrap();

        let db = manager_a.db.lock().await;
        let sa = storage::find_session_by_chat_id(&db, "group:dup", "mock", "inst-a")
            .unwrap()
            .expect("instance A session");
        let sb = storage::find_session_by_chat_id(&db, "group:dup", "mock", "inst-b")
            .unwrap()
            .expect("instance B session");
        assert_ne!(sa.id, sb.id, "each instance must own its session");

        // Both remain active; each archive holds only its own message.
        let ma = storage::get_messages(&db, &sa.id).unwrap();
        let mb = storage::get_messages(&db, &sb.id).unwrap();
        assert_eq!(ma.len(), 1);
        assert_eq!(ma[0].content, "from A");
        assert_eq!(mb.len(), 1);
        assert_eq!(mb[0].content, "from B");
    }

    #[tokio::test]
    async fn manager_passes_custom_prompt_to_agent() {
        let dir = tempfile::tempdir().unwrap();
        let config: RobitConfig = toml::from_str(
            r#"
            default_model = "test/text"
            [providers.test]
            base_url = "http://127.0.0.1:1"
            api_key = "sk-test"
            [[providers.test.models]]
            id = "text"
            [app.bot]
            system_prompt = "HOST PERSONA MARKER"
            "#,
        )
        .unwrap();
        let manager = make_manager(dir.path().to_path_buf(), config);
        assert_eq!(manager.custom_prompt.as_deref(), Some("HOST PERSONA MARKER"));

        // Spawn a session and verify the system prompt carries the persona.
        let (_tx, _frontend) = manager
            .get_or_create_session("group:9", "hi")
            .await
            .unwrap();
        // The Agent task owns the prompt; verify via the config plumbing that
        // spawn_session_agent received it (custom_prompt field above) — the
        // prompt content itself is covered by robit-agent prompt tests.
    }
}
