# robit-chatbot & robit-qq Design Specification

> **Date:** 2026-06-18
> **Status:** Draft
> **Related:** [[Frontend trait]], [[architecture]], [[robit-gui]], [[robit-feishu]]

## 1. Overview

**robit-chatbot** is a shared multi-session Bot infrastructure crate, and **robit-qq** is the QQ Bot platform implementation built on top of it. Together they enable the Robit AI agent to operate as a QQ Bot in group chats and private chats.

The design explicitly anticipates future platform additions (e.g., Feishu/Lark) by extracting all platform-agnostic logic into `robit-chatbot` behind a `PlatformAdapter` trait. Adding a new platform only requires implementing that trait.

### 1.1 Goals

- Enable Robit Agent to respond to QQ group chat and private chat messages via QQ Official Bot API
- **Multi-session**: each chat (group or user) has an independent Agent session with isolated conversation history
- **Session persistence**: chat history stored in SQLite, survives Bot restarts
- **Streaming output**: near-real-time message updates via smart-segmented text delivery
- **Tool confirmation**: inline confirmation messages with configurable auto-approve and timeout
- **Architecture reuse**: `robit-chatbot` shared base for future Feishu/Lark integration
- Maintain zero changes to `robit-agent` and `robit-ai` — purely new Frontend implementations

### 1.2 Non-Goals (MVP)

- QQ Guild (频道) support — group + private chat only
- Image/file message handling (text-only for MVP)
- Voice message support
- Multi-Bot account management (single Bot token)
- Webhook callback mode (WebSocket only)
- Feishu implementation (architecture reserved, not built)

---

## 2. Architecture

### 2.1 Workspace Structure

```
crates/
├── robit-chatbot/              ← New crate: multi-session Bot infrastructure
│   ├── Cargo.toml              # depends on robit-agent, robit-ai, rusqlite, tokio
│   └── src/
│       ├── lib.rs              # pub mod manager, adapter, frontend, confirmer, markdown
│       ├── manager.rs          # ChatbotManager<T: PlatformAdapter> — core orchestrator
│       ├── adapter.rs          # PlatformAdapter trait + PlatformCaps + types
│       ├── frontend.rs         # ChatbotFrontend — implements Frontend trait (per-session)
│       ├── confirmer.rs        # Confirmer — tool confirmation coordinator
│       └── markdown.rs         # Markdown sanitizer for platform-specific rendering
│
├── robit-qq/                   ← New crate: QQ Bot platform implementation
│   ├── Cargo.toml              # depends on robit-chatbot, tokio-tungstenite
│   └── src/
│       ├── lib.rs
│       ├── main.rs             # Binary entry point (standalone executable)
│       ├── platform.rs         # QqPlatformAdapter — implements PlatformAdapter
│       └── protocol.rs         # QQ WebSocket message protocol (opcodes, payloads)
```

### 2.2 Dependency Graph

```
robit-qq
  ├── depends on: robit-chatbot (PlatformAdapter trait, ChatbotManager)
  └── depends on: tokio-tungstenite (WebSocket client)

robit-chatbot
  ├── depends on: robit-agent (Frontend trait, Agent, tools, storage)
  ├── depends on: robit-ai (LlmClient, config)
  └── depends on: rusqlite (session persistence)

robit-agent (unchanged)
robit-ai (unchanged)
```

### 2.3 Design Approach: Layered Abstraction

```
┌──────────────────────────────────────────────────────┐
│                    robit-qq                           │
│                                                       │
│  main.rs → QqPlatformAdapter                          │
│             ├── QQ WebSocket 连接 (tokio-tungstenite)  │
│             ├── QQ 消息协议解析/序列化                  │
│             ├── QQ Token 鉴权                         │
│             └── 群聊/私聊路由                          │
│                                                       │
│  QqPlatformAdapter::connect()                         │
│      ↓                                                │
│  ChatbotManager::run(platform)                        │
└──────────────────────┬───────────────────────────────┘
                       │
┌──────────────────────┴───────────────────────────────┐
│                  robit-chatbot                         │
│                                                       │
│  PlatformAdapter trait ── 平台只需实现这个              │
│                                                       │
│  ChatbotManager<T: PlatformAdapter>                    │
│  ┌─────────────────────────────────────────────────┐  │
│  │ SessionManager（多会话生命周期）                    │  │
│  │   - chat_id → AgentHandle 映射                   │  │
│  │   - 会话创建/恢复/过期清理                         │  │
│  │   - SQLite 持久化                                 │  │
│  │                                                  │  │
│  │ Agent 池管理                                      │  │
│  │   ┌──────────┐ ┌──────────┐                      │  │
│  │   │ Session 1│ │ Session 2│ ...                  │  │
│  │   │ + Agent  │ │ + Agent  │                      │  │
│  │   │ +Frontend│ │ +Frontend│                      │  │
│  │   └──────────┘ └──────────┘                      │  │
│  │                                                  │  │
│  │ Confirmer（工具确认协调器）                         │  │
│  │   - 内联确认消息 + 超时机制                         │  │
│  │   - 确认关键字匹配                                 │  │
│  │                                                  │  │
│  │ Markdown 清洗器                                    │  │
│  │   - 平台能力感知的格式转换                           │  │
│  └─────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────┘
```

### 2.4 Relationship to Existing Architecture

```
robit-agent (Frontend trait)
    ├── robit-tui       (single user, single Frontend)
    ├── robit-gui       (multi-session, one GuiFrontend per session)
    ├── robit-chatbot   (NEW — multi-session Bot base)
    │       ├── robit-qq      (QQ Bot)
    │       └── robit-feishu (future, reserved)
```

`robit-qq` and `robit-feishu` each produce a standalone binary. They use `robit-chatbot`'s `ChatbotManager` internally, which handles all Agent/session lifecycle. The platform crate only implements `PlatformAdapter`.

---

## 3. Core Traits and Types

### 3.1 PlatformAdapter Trait

Defined in `robit-chatbot/src/adapter.rs`:

```rust
use async_trait::async_trait;
use robit_agent::error::Result;

/// Capabilities that vary by platform.
#[derive(Debug, Clone)]
pub struct PlatformCaps {
    /// Platform supports editing previously-sent messages (used for streaming updates).
    pub supports_edit: bool,
    /// Platform returns a message ID when sending (needed for edit).
    pub returns_msg_id: bool,
    /// Platform supports Markdown formatting in messages.
    pub supports_markdown: bool,
    /// Supported Markdown features (if supports_markdown).
    pub markdown_features: MarkdownFeatures,
    /// Maximum message length in characters (0 = no limit).
    pub max_message_length: usize,
}

#[derive(Debug, Clone, Default)]
pub struct MarkdownFeatures {
    pub headings: bool,
    pub bold: bool,
    pub italic: bool,
    pub code_blocks: bool,
    pub inline_code: bool,
    pub links: bool,
    pub unordered_lists: bool,
    pub ordered_lists: bool,
    pub blockquotes: bool,
    pub tables: bool,
    pub task_lists: bool,
    pub images: bool,
    pub strikethrough: bool,
}

/// Result of sending a message.
#[derive(Debug, Clone)]
pub struct SendResult {
    /// Platform-assigned message ID (for later editing).
    pub msg_id: String,
}

/// Sender information extracted from a platform event.
#[derive(Debug, Clone)]
pub struct SenderInfo {
    pub user_id: String,
    pub chat_id: String,       // group_id for group chat, user_id for private chat
    pub chat_type: ChatType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatType {
    Private,
    Group,
}

/// A parsed chat message from the platform.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub text: String,
    pub sender: SenderInfo,
}

/// Platform events that ChatbotManager processes.
#[derive(Debug)]
pub enum PlatformEvent {
    /// A chat message from a user.
    Message(ChatMessage),
    /// Connection was lost (triggers reconnect).
    Disconnected,
    /// Other event (e.g., bot invited to group, kicked, etc.).
    Other(serde_json::Value),
}

/// The trait every chat platform must implement.
#[async_trait]
pub trait PlatformAdapter: Send + Sync + 'static {
    /// Platform capabilities. Used by ChatbotFrontend for streaming strategy
    /// and by the Markdown sanitizer.
    fn capabilities() -> PlatformCaps;

    /// Establish connection to the platform. Returns self once connected.
    async fn connect(config: &Self::Config) -> Result<Self>
    where
        Self: Sized;

    /// Send a text message to a chat. Returns the platform message ID.
    async fn send_message(&self, chat_id: &str, text: &str) -> Result<SendResult>;

    /// Edit a previously-sent message. Default implementation falls back
    /// to send_message for platforms that don't support editing.
    async fn edit_message(&self, chat_id: &str, msg_id: &str, text: &str) -> Result<()> {
        let _ = self.send_message(chat_id, text).await;
        Ok(())
    }

    /// Receive the next platform event. Blocks until an event arrives.
    async fn recv_event(&self) -> Result<PlatformEvent>;
}
```

### 3.2 ChatbotManager

Defined in `robit-chatbot/src/manager.rs`:

```rust
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use rusqlite::Connection;

use robit_agent::Agent;
use robit_ai::config::RobitConfig;
use robit_ai::LlmClient;

use crate::adapter::PlatformAdapter;
use crate::confirmer::Confirmer;
use crate::frontend::ChatbotFrontend;

/// Handle to a running Agent instance for one chat.
pub struct AgentHandle {
    /// Send messages to the Agent loop.
    pub message_tx: mpsc::Sender<FrontendMessage>,
    /// The session ID (UUID) for this chat.
    pub session_id: String,
    /// Last activity timestamp (for session expiry).
    pub last_active_at: Instant,
}

/// Core orchestrator for multi-session Bot operations.
pub struct ChatbotManager<T: PlatformAdapter> {
    /// Platform adapter instance.
    platform: T,

    /// Active Agent instances, keyed by chat_id.
    agents: Mutex<HashMap<String, AgentHandle>>,

    /// SQLite connection for session persistence.
    db: Arc<Mutex<Connection>>,

    /// Loaded configuration.
    config: RobitConfig,

    /// Working directory for tool execution.
    working_dir: PathBuf,

    /// Shared LLM client (all sessions reuse).
    llm_client: Arc<LlmClient>,

    /// Shared tool registry.
    tool_registry: Arc<ToolRegistry>,

    /// Shared skill registry.
    skill_registry: Arc<SkillRegistry>,

    /// Auto-approve all tool calls.
    auto_approve: bool,

    /// Context window from resolved model.
    context_window: Option<u64>,

    /// Tool confirmation coordinator.
    confirmer: Confirmer,

    /// Session idle timeout (inactive sessions are cleaned up).
    session_timeout: Duration,
}

impl<T: PlatformAdapter> ChatbotManager<T> {
    /// Create a new ChatbotManager.
    pub fn new(
        platform: T,
        config: RobitConfig,
        working_dir: PathBuf,
        llm_client: Arc<LlmClient>,
        tool_registry: Arc<ToolRegistry>,
        skill_registry: Arc<SkillRegistry>,
    ) -> Result<Self>;

    /// Main event loop. Connects to the platform, then processes events forever.
    pub async fn run(&self) -> Result<()>;

    /// Process a single incoming chat message.
    async fn handle_message(&self, msg: ChatMessage);

    /// Get or create an Agent session for a chat_id.
    /// Returns the message_tx sender (cloned) for sending messages to the Agent.
    async fn get_or_create_session(&self, chat_id: &str) -> Result<mpsc::Sender<FrontendMessage>>;

    /// Create a new Agent for a chat_id.
    async fn create_session(&self, chat_id: &str) -> Result<AgentHandle>;

    /// Restore an Agent from persisted DB history.
    async fn restore_session(&self, chat_id: &str) -> Result<AgentHandle>;

    /// Periodically clean up inactive sessions.
    async fn cleanup_loop(&self);
}
```

### 3.3 ChatbotFrontend

Defined in `robit-chatbot/src/frontend.rs`:

```rust
use std::sync::Arc;
use tokio::sync::Mutex;

/// Per-session Frontend trait implementation for Bot platforms.
///
/// Each chat (group or private) gets its own ChatbotFrontend instance.
/// TextDelta events are buffered and flushed in smart segments.
pub struct ChatbotFrontend {
    /// The chat this frontend belongs to (group_id or user_id).
    chat_id: String,

    /// Platform message sender (shared across all frontends).
    platform_sender: Arc<dyn PlatformSender>,

    /// Tool confirmation coordinator (shared).
    confirmer: Arc<Confirmer>,

    /// Streaming text buffer.
    buffer: Mutex<String>,

    /// ID of the last message sent (for edit-based streaming updates).
    last_msg_id: Mutex<Option<String>>,

    /// Auto-approve flag.
    auto_approve: bool,
}

/// Abstracted message sending capability (platform-agnostic).
#[async_trait]
pub trait PlatformSender: Send + Sync {
    async fn send(&self, chat_id: &str, text: &str) -> Result<SendResult>;
    async fn edit(&self, chat_id: &str, msg_id: &str, text: &str) -> Result<()>;
    fn capabilities(&self) -> PlatformCaps;
}
```

`ChatbotFrontend` implements `robit_agent::frontend::Frontend`:

```rust
#[async_trait]
impl Frontend for ChatbotFrontend {
    async fn on_event(&self, event: AgentEvent) -> Result<()> {
        match event {
            AgentEvent::TextDelta(delta) => {
                let mut buffer = self.buffer.lock().await;
                buffer.push_str(&delta);
                self.maybe_flush(&mut buffer).await;
            }
            AgentEvent::ToolCallRequested { tool_call_id, name, arguments } => {
                // Flush any buffered text first
                self.flush_buffer().await;
                // Tool call info is handled within the confirmation flow
            }
            AgentEvent::ToolCallResult { tool_call_id, result } => {
                // Send result as a system notice (optional, could skip for brevity)
            }
            AgentEvent::TurnComplete => {
                self.flush_buffer().await;
            }
            AgentEvent::Error(e) => {
                self.flush_buffer().await;
                self.platform_sender.send(&self.chat_id, &format!("❌ Error: {}", e)).await;
            }
            AgentEvent::SkillTriggered { name, description } => {
                // Optional: send a brief notice
            }
        }
        Ok(())
    }

    async fn request_tool_confirmation(&self, info: &ToolCallInfo) -> Result<bool> {
        self.confirmer.request(&self.chat_id, info, self.auto_approve).await
    }
}
```

### 3.4 Confirmer

Defined in `robit-chatbot/src/confirmer.rs`:

```rust
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// Tool confirmation coordinator for Bot platforms.
///
/// Unlike GUI which has dialog boxes, Bot confirmation happens via inline
/// chat messages. The Confirmer sends a confirmation prompt, then waits
/// for the user to reply with an approve/reject keyword.
pub struct Confirmer {
    /// Pending confirmations: key = "chat_id:tool_call_id"
    /// Uses std::sync::Mutex because the lock is held only briefly for HashMap ops.
    pending: Mutex<HashMap<String, PendingConfirmation>>,
    /// Platform sender for confirmation prompts.
    platform_sender: Arc<dyn PlatformSender>,
    /// Timeout for waiting for user response.
    timeout: Duration,
}

struct PendingConfirmation {
    sender: oneshot::Sender<bool>,
    created_at: Instant,
    chat_id: String,
}

/// Keywords that trigger approval or rejection.
#[derive(Debug, Clone)]
pub struct ConfirmKeywords {
    pub approve: Vec<String>,
    pub reject: Vec<String>,
}

impl Default for ConfirmKeywords {
    fn default() -> Self {
        Self {
            approve: vec![
                "确认".into(), "同意".into(), "yes".into(), "y".into(),
                "approve".into(), "ok".into(), "允许".into(),
            ],
            reject: vec![
                "取消".into(), "拒绝".into(), "no".into(), "n".into(),
                "reject".into(), "cancel".into(), "deny".into(),
            ],
        }
    }
}

impl Confirmer {
    /// Create a new Confirmer.
    pub fn new(platform_sender: Arc<dyn PlatformSender>, timeout: Duration) -> Self;

    /// Request tool confirmation. Sends a prompt message and waits for response.
    /// Returns true if approved, false if rejected or timed out.
    pub async fn request(
        &self,
        chat_id: &str,
        info: &ToolCallInfo,
        auto_approve: bool,
    ) -> Result<bool>;

    /// Check if a user message is a confirmation response for a pending request.
    /// Returns Some(approved) if the message matches, None otherwise.
    pub fn check_confirmation_response(
        &self,
        chat_id: &str,
        text: &str,
    ) -> Option<bool>;

    /// Periodically clean up expired pending confirmations.
    pub async fn cleanup_expired(&self);
}
```

### 3.5 ConfirmKeywords — Configuration

```toml
# In config.toml (optional section, under [app.bot])
[app.bot.confirm_keywords]
approve = ["确认", "同意", "yes", "y", "approve", "ok", "允许"]
reject = ["取消", "拒绝", "no", "n", "reject", "cancel", "deny"]

[app.bot]
confirm_timeout_secs = 60    # default
```

---

## 4. Session Management

### 4.1 Chat ID Mapping

```
chat_id schema:
  "group:{group_openid}"   → group chat
  "private:{user_openid}"  → private chat (C2C)

session_id: UUID v4 (generated by Agent::new)
```

The `agents` map in `ChatbotManager` is keyed by `chat_id`. Session lookup is O(1).

### 4.2 Session Lifecycle

```
User sends message in chat
        │
        ▼
ChatbotManager::handle_message(msg)
        │
        ▼
  chat_id in agents map?
   ├── Yes → use existing AgentHandle, update last_active_at
   │
   └── No → check DB for persisted session
            ├── Found → restore_session(): create Agent, load history from DB
            └── Not found → create_session(): new Agent + new DB session
                    │
                    ▼
            spawn tokio task: agent.run(message_rx)
            insert into agents map
                    │
                    ▼
            agent.message_tx.send(UserInput(msg.text))
```

### 4.3 Session Expiry

Inactive sessions (no messages for `session_timeout`, default 30 minutes) are cleaned up:

```
cleanup_loop (runs every 5 minutes):
    for each (chat_id, handle) in agents:
        if now - handle.last_active_at > session_timeout:
            agents.remove(chat_id)
            // Agent task will naturally exit when message_rx is dropped
            // DB session record is preserved (persistence)
```

Session timeout is configurable in `config.toml`:

```toml
[app]
session_timeout_minutes = 30  # default
```

### 4.4 Session Restoration from DB

When a user messages a chat that had a previous session (stored in DB), the Agent is re-created with the persisted conversation history:

1. Load session record from `sessions` table by `chat_id → session_id` mapping
2. Load messages from `messages` table
3. Create new `ChatbotFrontend` for the chat
4. Create `Agent` with the loaded message history (as `AgentSession.history`)
5. Spawn Agent task
6. Register in `agents` map

Note: This requires a `chat_id` column in the `sessions` table (see §7 Database Schema).

---

## 5. Streaming Output Strategy

### 5.1 Smart Segmentation

Rather than sending on fixed character counts (which would cut mid-word or mid-code-block), the streaming buffer uses **natural boundary detection**:

```
TextDelta arrives
    ↓
Append to buffer
    ↓
buffer length ≥ flush_threshold (default: 200 chars)?
    ├── Yes → find nearest natural break point:
    │         Priority order:
    │         1. Paragraph break (\n\n)          ← best: between paragraphs
    │         2. Code block end (```)            ← don't cut inside code blocks
    │         3. Line break (\n)                 ← end of a line
    │         4. Sentence end (. 。! ！? ？)       ← end of a sentence
    │         5. Space after word                ← fallback
    │
    │         Split buffer at the break point.
    │         Send the first segment.
    │         Keep the remainder in buffer.
    │
    └── No → continue accumulating
    ↓
TurnComplete arrives
    ↓
Flush all remaining buffer content
```

### 5.2 Edit-Based Streaming (Platforms with `supports_edit = true`)

```
First segment → send_message() → get msg_id
Subsequent segments → edit_message(chat_id, msg_id, accumulated_full_text)
TurnComplete → final edit_message() with complete text
```

This creates a "growing message" effect where the user sees the response building up in-place.

### 5.3 Fallback Streaming (Platforms without edit support)

```
First segment → send_message() with trailing "..." indicator
Subsequent segments → send_message() as separate messages
TurnComplete → send final segment without "..." indicator
```

### 5.4 Platform-Specific Streaming Configuration

```rust
impl PlatformCaps {
    pub fn qq() -> Self {
        Self {
            supports_edit: true,     // QQ supports message editing
            returns_msg_id: true,
            supports_markdown: true,
            markdown_features: MarkdownFeatures::qq(),
            max_message_length: 2000,
        }
    }

    pub fn feishu() -> Self {  // reserved
        Self {
            supports_edit: true,
            returns_msg_id: true,
            supports_markdown: true,
            markdown_features: MarkdownFeatures::feishu(),
            max_message_length: 30000,  // Feishu has much larger limit
        }
    }
}
```

The `ChatbotFrontend` adapts its behavior based on `PlatformCaps` — no per-platform code needed.

---

## 6. Markdown Sanitization

### 6.1 Problem

LLM outputs are in Markdown. QQ Bot supports a subset of Markdown. Unsupported syntax (tables, task lists, HTML) must be converted or stripped before sending.

### 6.2 Sanitizer Design

Located in `robit-chatbot/src/markdown.rs`:

```rust
/// Convert LLM Markdown output to a platform-compatible format.
pub fn sanitize_markdown(text: &str, features: &MarkdownFeatures) -> String {
    // 1. Strip HTML tags (never supported by Bot platforms)
    // 2. Convert tables → aligned plain text (if !features.tables)
    // 3. Convert task lists → unordered lists (if !features.task_lists)
    // 4. Convert image syntax to "[Image: alt_text]" (if !features.images)
    // 5. Strip unsupported inline formatting
    // 6. Preserve supported syntax as-is
    // 7. Truncate code blocks that exceed max_message_length
}
```

The sanitizer runs automatically in `ChatbotFrontend::maybe_flush()` before sending.

### 6.3 QQ Markdown Features

Based on QQ Official Bot API capabilities:

| Feature | Supported | Notes |
|---------|-----------|-------|
| Headings `#`–`####` | ✅ | |
| Bold `**text**` | ✅ | |
| Italic `*text*` | ✅ | |
| Code blocks ` ``` ` | ✅ | Language tag stripped |
| Inline code `` ` `` | ✅ | |
| Links `[text](url)` | ✅ | |
| Unordered lists `- ` | ✅ | |
| Ordered lists `1. ` | ✅ | |
| Blockquotes `> ` | ✅ | |
| Strikethrough `~~text~~` | ✅ | |
| Tables | ❌ | Convert to aligned text |
| Task lists `- [ ]` | ❌ | Convert to unordered list |
| HTML tags | ❌ | Strip entirely |
| Images `![]()` | ❌ | Convert to `[Image: alt]` |
| Horizontal rules `---` | ❌ | Strip |

### 6.4 Future Feishu Markdown Features (reserved)

Feishu uses a different message format (block-based JSON). When Feishu is implemented, the sanitizer will support a "Feishu" output mode that converts Markdown to Feishu's block format. The `PlatformCaps` → `markdown_features` mapping drives this conversion.

---

## 7. Tool Confirmation Flow

### 7.1 Interaction Sequence

```
Agent decides to call bash/write/edit
    │
    ▼
ChatbotFrontend::request_tool_confirmation(info)
    │
    ▼
Confirmer::request(chat_id, info, auto_approve)
    │
    ├── auto_approve == true? → return true immediately
    │
    ├── Send confirmation prompt to chat:
    │   ┌──────────────────────────────────────┐
    │   │ ⚠️ 需要确认工具调用                    │
    │   │                                       │
    │   │ 工具: bash                            │
    │   │ 参数: rm -rf /tmp/cache               │
    │   │                                       │
    │   │ 回复 "确认" 或 "取消"                   │
    │   │ (60秒内有效)                           │
    │   └──────────────────────────────────────┘
    │
    ├── Register in pending map:
    │   key = "{chat_id}:{tool_call_id}"
    │   value = PendingConfirmation { sender, created_at, chat_id }
    │
    ├── Wait on oneshot::Receiver<bool>
    │   (with tokio::time::timeout)
    │
    ├── On response → send result notice, return bool
    ├── On timeout → send "⏰ 已超时，操作已取消", return false
    └── Cleanup: remove from pending map
```

### 7.2 Message Routing

When a user message arrives in `ChatbotManager::handle_message()`, it must be checked BEFORE routing to the Agent:

```rust
async fn handle_message(&self, msg: ChatMessage) {
    let chat_id = &msg.sender.chat_id;
    let text = msg.text.trim().to_lowercase();

    // Check if this is a confirmation response
    if let Some(approved) = self.confirmer.check_confirmation_response(chat_id, &text) {
        // Route to Confirmer, NOT to Agent
        // The oneshot sender will unblock the waiting request_tool_confirmation()
        return;
    }

    // Normal message → route to Agent
    self.get_or_create_session(chat_id).await
        .message_tx.send(FrontendMessage::UserInput(msg.text))
        .await;
}
```

### 7.3 Confirmation Response Detection

```rust
impl Confirmer {
    pub fn check_confirmation_response(&self, chat_id: &str, text: &str) -> Option<bool> {
        let pending = self.pending.lock().unwrap();

        // Find the first pending confirmation for this chat
        let key_prefix = format!("{}:", chat_id);
        let matching_key = pending.keys().find(|k| k.starts_with(&key_prefix))?;

        let text_lower = text.trim().to_lowercase();

        if self.keywords.approve.iter().any(|kw| text_lower == *kw) {
            let entry = pending.remove(matching_key)?;
            let _ = entry.sender.send(true);
            Some(true)
        } else if self.keywords.reject.iter().any(|kw| text_lower == *kw) {
            let entry = pending.remove(matching_key)?;
            let _ = entry.sender.send(false);
            Some(false)
        } else {
            None // Not a confirmation keyword
        }
    }
}
```

Only exact keyword matches trigger confirmation routing. This means:
- If there IS a pending confirmation: "确认" / "取消" routes to Confirmer
- If there is NO pending confirmation: "确认" / "取消" routes to Agent as normal dialogue
- Other messages are always routed to Agent

---

## 8. Database Schema

### 8.1 Schema Extension

The existing `storage.rs` schema in `robit-agent` supports sessions and messages. For Bot platforms, we extend it with a `chat_id` mapping:

```sql
-- Existing tables (unchanged)
CREATE TABLE sessions (
    id          TEXT PRIMARY KEY,     -- UUID v4 (session_id)
    title       TEXT NOT NULL,
    model       TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    is_active   INTEGER DEFAULT 1
);

CREATE TABLE messages (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id   TEXT NOT NULL REFERENCES sessions(id),
    role         TEXT NOT NULL,
    content      TEXT NOT NULL,
    tool_name    TEXT,
    tool_call_id TEXT,
    tokens       INTEGER,
    created_at   TEXT NOT NULL
);

-- NEW: chat_id mapping for Bot platforms
-- Allows session lookup by platform chat identifier
ALTER TABLE sessions ADD COLUMN chat_id TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_chat_id
    ON sessions(chat_id) WHERE chat_id IS NOT NULL;
```

### 8.2 Session ↔ Chat Mapping

| Column | Example Value | Description |
|--------|--------------|-------------|
| `sessions.id` | `a1b2c3d4-...` | Agent session UUID |
| `sessions.chat_id` | `group:abc123` | Platform chat identifier |
| `sessions.title` | `技术讨论群` | Auto-generated from first message |

The `chat_id` column is `NULL` for non-Bot frontends (TUI, GUI). The unique index ensures one session per chat.

### 8.3 Storage Scope

Bot platforms use **project-local** storage by default:

```
.robit/memory/robit.db    # in working directory
```

`global_storage` config is also supported, resolving to `~/.robit/memory/robit.db`.

---

## 9. QQ Protocol Implementation

### 9.1 WebSocket Gateway Protocol

QQ Official Bot uses a Discord-like WebSocket gateway:

```
Connection: wss://api.sgroup.qq.com/gateway

Payload format:
{
  "op": 0-13,       // Opcode
  "d": {},           // Data payload
  "s": 42,           // Sequence number (for resume)
  "t": "MESSAGE_CREATE"  // Event type (for op=0 Dispatch)
}
```

### 9.2 Opcodes

| Opcode | Name | Description |
|--------|------|-------------|
| 0 | Dispatch | Server pushes an event |
| 1 | Heartbeat | Client sends heartbeat |
| 2 | Identify | Client sends authentication |
| 6 | Resume | Client resumes a broken connection |
| 7 | Reconnect | Server asks client to reconnect |
| 9 | Invalid Session | Server indicates session is invalid |
| 10 | Hello | Server sends heartbeat interval |
| 11 | Heartbeat ACK | Server acknowledges heartbeat |
| 13 | Client Status | Client updates status |

### 9.3 Connection Lifecycle

```
Client                              Server
  │                                    │
  │──── wss://api.sgroup.qq.com ──────▶│
  │                                    │
  │◀──── Hello (op=10, heartbeat_interval=41250ms) ────│
  │                                    │
  │──── Identify (op=2, token, intents) ──▶│
  │                                    │
  │◀──── Ready (op=0, t=READY) ────────│
  │                                    │
  │  ═══════ Connected ═══════════════ │
  │                                    │
  │──── Heartbeat (op=1, seq) ────────▶│  (every ~41s)
  │◀──── Heartbeat ACK (op=11) ───────│
  │                                    │
  │◀──── Dispatch (op=0, t=MESSAGE_CREATE) ──│
  │◀──── Dispatch (op=0, t=AT_MESSAGE_CREATE) ──│
  │                                    │
```

### 9.4 QQ-Specific Intents

```rust
pub enum QqIntent {
    GuildMessages = 1 << 0,       // Guild messages (not used in MVP)
    GuildMembers = 1 << 1,        // Guild member events (not used)
    DirectMessage = 1 << 12,      // C2C private messages
    GroupAtMessage = 1 << 25,     // Group @ messages
    Interaction = 1 << 26,        // Interaction events
}
```

MVP intents: `DirectMessage | GroupAtMessage`

### 9.5 Message Types Received

| Event Type | Chat Type | Description |
|------------|-----------|-------------|
| `C2C_MESSAGE_CREATE` | Private | Private chat message |
| `GROUP_AT_MESSAGE_CREATE` | Group | Group message @ the Bot |
| `MESSAGE_CREATE` | Group | Group message (if bot has permission) |

### 9.6 QqPlatformAdapter Structure

```rust
// In robit-qq/src/platform.rs

pub struct QqPlatformAdapter {
    /// WebSocket connection (tokio-tungstenite).
    ws: Mutex<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    /// Bot token (from config).
    token: String,
    /// Bot App ID (from config).
    app_id: String,
    /// Bot secret (from config).
    app_secret: String,
    /// Last sequence number received (for resume).
    last_seq: Mutex<Option<u64>>,
    /// Session ID from Hello (for resume).
    session_id: Mutex<Option<String>>,
    /// Heartbeat interval from Hello.
    heartbeat_interval: Duration,
    /// Event channel (recv_event reads from this).
    event_rx: Mutex<mpsc::Receiver<PlatformEvent>>,
    /// Platform capabilities.
    caps: PlatformCaps,
}

impl PlatformAdapter for QqPlatformAdapter {
    fn capabilities() -> PlatformCaps { PlatformCaps::qq() }

    async fn connect(config: &QqConfig) -> Result<Self> {
        // 1. Connect WebSocket to wss://api.sgroup.qq.com/gateway
        // 2. Wait for Hello (op=10), extract heartbeat_interval
        // 3. Send Identify (op=2) with token and intents
        // 4. Wait for Ready (op=0, t=READY)
        // 5. Spawn heartbeat task
        // 6. Spawn dispatch task: reads WS messages, converts to PlatformEvent, sends to event_rx
        // 7. Return Self
    }

    async fn send_message(&self, chat_id: &str, text: &str) -> Result<SendResult> {
        // HTTP POST to https://api.sgroup.qq.com/v2/groups/{group_id}/messages
        // or /v2/users/{user_id}/messages for C2C
        // Returns message ID from response
    }

    async fn recv_event(&self) -> Result<PlatformEvent> {
        self.event_rx.lock().await.recv().await
            .ok_or_else(|| AgentError::InternalError("Event channel closed".into()))
    }
}
```

### 9.7 QQ Configuration

```toml
# In config.toml
[channels.qq_bot]
app_id = "123456789"
app_secret = "${QQ_BOT_SECRET}"
bot_token = "${QQ_BOT_TOKEN}"

# Bot-specific app config
[app.qq]
# Confirm keywords for tool confirmation
confirm_timeout_secs = 60
session_timeout_minutes = 30
```

### 9.8 Dependencies (robit-qq Cargo.toml)

```toml
[dependencies]
robit-chatbot = { path = "../robit-chatbot" }
robit-agent.workspace = true
robit-ai.workspace = true
tokio.workspace = true
tokio-tungstenite = "0.24"
tokio-util = "0.7"
futures-util = "0.3"
serde.workspace = true
serde_json.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
clap.workspace = true
```

---

## 10. Main Entry Point

```rust
// robit-qq/src/main.rs

use clap::Parser;
use robit_ai::config::load_config;
use robit_ai::LlmClient;
use robit_chatbot::ChatbotManager;

#[derive(Debug, Parser)]
#[command(name = "robit-qq")]
#[command(about = "Robit AI Agent - QQ Bot")]
#[command(version)]
struct Cli {
    /// Working directory for the agent
    #[arg(long, short = 'w')]
    workdir: Option<std::path::PathBuf>,

    /// Use global storage for session database
    #[arg(long)]
    global_storage: bool,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("robit_qq=info".parse().unwrap())
                .add_directive("robit_chatbot=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    let config = load_config(cli.workdir.as_deref())
        .expect("Failed to load config.toml");

    let llm_client = Arc::new(
        LlmClient::from_config(&config, None)
            .expect("Failed to initialize LLM client"),
    );

    // Bootstrap tools and skills
    let base_tool_names = ["read", "bash", "write", "edit"];
    let bootstrap_result = robit_agent::bootstrap(
        &config,
        &cli.workdir.unwrap_or_else(|| std::env::current_dir().unwrap()),
        &base_tool_names,
    );
    robit_agent::log_skill_errors(&bootstrap_result.skill_load_errors);

    // Create QQ platform adapter
    let qq_config = QqConfig::from_config(&config)
        .expect("QQ Bot config not found");
    let platform = QqPlatformAdapter::connect(&qq_config).await
        .expect("Failed to connect to QQ");

    // Create manager and run
    let manager = ChatbotManager::new(
        platform,
        config,
        cli.workdir,
        llm_client,
        bootstrap_result.tool_registry,
        bootstrap_result.skill_registry,
    ).expect("Failed to create ChatbotManager");

    manager.run().await.expect("ChatbotManager error");
}
```

---

## 11. Future: Feishu/Lark Integration

### 11.1 What's Already Handled

When Feishu integration is added, the following components are **already ready** with zero changes:

- `ChatbotManager` — session lifecycle, Agent management, DB persistence
- `ChatbotFrontend` — streaming buffer, smart segmentation, tool confirmation delegation
- `Confirmer` — inline confirmation with keywords and timeout
- `markdown.rs` — platform-aware sanitization (add `MarkdownFeatures::feishu()`)
- `PlatformCaps` — add `PlatformCaps::feishu()` variant

### 11.2 What's Needed for Feishu

Only a new `robit-feishu` crate implementing `PlatformAdapter`:

```
crates/robit-feishu/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── main.rs
    ├── platform.rs     # FeishuPlatformAdapter
    └── protocol.rs     # Feishu WebSocket/Lark protocol
```

Key differences from QQ:
- Feishu uses a different WebSocket gateway (`wss://open.feishu.cn/...`)
- Message format is block-based JSON (not plain text Markdown)
- `markdown_features` will differ (Feishu supports more Markdown)
- Auth uses App ID + App Secret (OAuth2 token exchange)

---

## 12. Configuration

### 12.1 New Config Sections

`channels` is a new top-level section for communication channel configurations (QQ Bot, Feishu, etc.), separate from `providers` which configures LLM model providers.

```toml
# QQ Bot channel
[channels.qq_bot]
app_id = "123456789"
app_secret = "${QQ_BOT_SECRET}"
bot_token = "${QQ_BOT_TOKEN}"

# Bot-specific app settings (optional, with defaults shown)
[app.bot]
auto_approve = false                  # Auto-approve all tool calls
confirm_timeout_secs = 60             # Tool confirmation timeout
session_timeout_minutes = 30          # Idle session expiry

[app.bot.confirm_keywords]
approve = ["确认", "同意", "yes", "y", "approve", "ok", "允许"]
reject = ["取消", "拒绝", "no", "n", "reject", "cancel", "deny"]
```

### 12.2 Environment Variables

```bash
# ~/.robit/.env
QQ_BOT_TOKEN=your_bot_token_here
QQ_BOT_SECRET=your_app_secret_here
```

---

## 13. Testing Strategy

| Level | Tool | Scope |
|-------|------|-------|
| Rust unit tests | `cargo test -p robit-chatbot` | Markdown sanitizer, Confirmer logic, session mapping, streaming buffer segmentation |
| Rust unit tests | `cargo test -p robit-qq` | QQ protocol message parsing, payload serialization, opcode handling |
| Integration tests | `cargo test -p robit-chatbot` | ChatbotFrontend + mock PlatformAdapter, ChatbotManager session lifecycle |
| Manual testing | QQ sandbox Bot | End-to-end group/private chat interaction, tool confirmation flow, streaming display |

Mock `PlatformAdapter` for integration testing:

```rust
struct MockPlatform {
    sent_messages: Mutex<Vec<(String, String)>>,
    events: Mutex<VecDeque<PlatformEvent>>,
}

// Implements PlatformAdapter with in-memory queues
```

---

## 14. Key Design Decisions Log

| # | Decision | Rationale |
|---|----------|-----------|
| 1 | Separate `robit-chatbot` crate | QQ and Feishu share ~80% of logic; extracting avoids duplication |
| 2 | `PlatformAdapter` trait | Clean abstraction boundary: platform code only handles connection + message format |
| 3 | Multi-Agent per process | Each chat has independent conversation; matches robit-gui's model |
| 4 | Smart segmentation for streaming | Fixed-length cutting would break Markdown; natural boundary detection preserves readability |
| 5 | ChatbotFrontend per session | Matches GuiFrontend pattern; one `Frontend` impl instance per chat |
| 6 | Confirmer with oneshot channels | Same pattern as GuiFrontend; blocking wait with timeout |
| 7 | Exact keyword matching for confirm | Prevents false positives; only intercepts when pending confirmation exists |
| 8 | No official Rust QQ SDK | Self-implement WebSocket protocol via tokio-tungstenite; protocol is well-documented |
| 9 | Extend sessions table with chat_id | Clean mapping without schema redesign; NULL for non-Bot frontends |
| 10 | Project-local DB by default | Matches robit-gui; global_storage config override available |
| 11 | Text-only MVP | Image/file handling adds significant complexity; can be layered on later |
| 12 | No Guild/频道 support in MVP | Group + private chat covers 90% of use cases; Guild requires additional intents |
