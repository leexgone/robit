//! Session and message storage helpers.

use std::path::{Path, PathBuf};

use async_openai::types::chat::{
    ChatCompletionRequestAssistantMessage, ChatCompletionRequestMessage,
    ChatCompletionRequestToolMessage, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent,
};
use rusqlite::{params, Connection, Result as SqliteResult};
use serde::{Deserialize, Serialize};

use crate::datetime::current_timestamp;
use crate::error::Result;

const ROBIT_DIR: &str = ".robit";
const MEMORY_DIR: &str = "memory";
const DB_FILE: &str = "robit.db";

/// Resolve the session database path for a working directory and storage scope.
pub fn resolve_db_path(working_dir: &Path, global_storage: bool) -> Result<PathBuf> {
    if global_storage {
        let home = dirs::home_dir().ok_or_else(|| {
            crate::error::AgentError::InternalError("Cannot determine home directory".to_string())
        })?;
        Ok(home.join(ROBIT_DIR).join(MEMORY_DIR).join(DB_FILE))
    } else {
        Ok(working_dir.join(ROBIT_DIR).join(MEMORY_DIR).join(DB_FILE))
    }
}

/// Session metadata returned to frontends.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: String,
    /// Platform chat identifier (None for GUI/TUI, Some for Bot platforms).
    pub chat_id: Option<String>,
    pub title: String,
    pub model: String,
    /// Which frontend created the session: "gui" | "tui" | "qq" | "feishu".
    pub source: String,
    pub status: String, // "idle" | "ready" | "running"
    pub created_at: String,
    pub updated_at: String,
}

/// Message data returned to frontends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageData {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_info: Option<serde_json::Value>,
    pub created_at: String,
}

/// Tool call info for storage in message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallInfoData {
    pub tool_call_id: String,
    pub name: String,
    pub arguments: String,
    pub status: String,
    pub output: Option<String>,
    pub requires_confirm: bool,
}

/// Current schema version. Increment when the schema changes.
const CURRENT_SCHEMA_VERSION: i32 = 3;

// ============================================================================
// Memory data structures
// ============================================================================

/// Type of memory entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MemoryType {
    /// Objective fact (e.g., "User likes Rust").
    Fact,
    /// User preference (e.g., "Prefer deepseek-chat").
    Preference,
    /// Note or documentation (e.g., "Project directory structure").
    Note,
    /// Task record (e.g., "Completed XX last time").
    Task,
    /// Custom type with a name.
    Custom(String),
}

impl MemoryType {
    pub fn as_str(&self) -> &str {
        match self {
            MemoryType::Fact => "fact",
            MemoryType::Preference => "preference",
            MemoryType::Note => "note",
            MemoryType::Task => "task",
            MemoryType::Custom(s) => s,
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "fact" => MemoryType::Fact,
            "preference" => MemoryType::Preference,
            "note" => MemoryType::Note,
            "task" => MemoryType::Task,
            _ => MemoryType::Custom(s.to_string()),
        }
    }
}

/// A memory entry stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    /// Unique ID (UUID v4).
    pub id: String,
    /// Associated session ID (optional).
    pub session_id: Option<String>,
    /// Associated chat ID (for Bot platforms, optional).
    pub chat_id: Option<String>,
    /// Type of memory.
    pub memory_type: MemoryType,
    /// Short title for the memory (for retrieval).
    pub title: String,
    /// Full content of the memory.
    pub content: String,
    /// Tags for categorization and filtering.
    pub tags: Vec<String>,
    /// Soft deletion flag.
    pub is_active: bool,
    /// ISO 8601 creation timestamp.
    pub created_at: String,
    /// ISO 8601 last update timestamp.
    pub updated_at: String,
}

/// Filter for memory queries.
#[derive(Debug, Clone, Default)]
pub struct MemoryFilter {
    /// Filter by memory type.
    pub memory_type: Option<MemoryType>,
    /// Filter by tags (any match).
    pub tags: Option<Vec<String>>,
    /// Filter by session ID.
    pub session_id: Option<String>,
    /// Filter by chat ID (Bot platforms).
    pub chat_id: Option<String>,
    /// Filter by creation time (only memories created after this time).
    pub since: Option<String>,
    /// Only return active memories (default: true).
    pub only_active: bool,
}

impl Memory {
    /// Create a new memory with the given details.
    pub fn new(
        title: String,
        content: String,
        memory_type: MemoryType,
        tags: Vec<String>,
    ) -> Self {
        let now = current_timestamp();
        Memory {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: None,
            chat_id: None,
            memory_type,
            title,
            content,
            tags,
            is_active: true,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// Attach a session ID to this memory.
    pub fn with_session_id(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Attach a chat ID to this memory.
    pub fn with_chat_id(mut self, chat_id: String) -> Self {
        self.chat_id = Some(chat_id);
        self
    }
}

/// Initialize the database: create tables if needed, then run migrations.
///
/// This is the single entry point used by all frontends. It auto-detects
/// fresh databases (version 0) and existing databases, running the migration
/// chain to bring them up to [`CURRENT_SCHEMA_VERSION`].
pub fn init_db(conn: &Connection) -> SqliteResult<()> {
    ensure_meta_table(conn)?;

    let version = read_schema_version(conn)?;

    if version == 0 {
        // Fresh database — create everything at the current version in one shot.
        create_all_tables(conn)?;
        write_schema_version(conn, CURRENT_SCHEMA_VERSION)?;
        tracing::info!(
            "Database initialized at schema v{}",
            CURRENT_SCHEMA_VERSION
        );
        return Ok(());
    }

    migrate(conn, version, CURRENT_SCHEMA_VERSION)?;
    Ok(())
}

/// Detect the schema version of an existing database.
///
/// Returns:
/// - `0` for a truly fresh database (no `sessions` table, no recorded version).
/// - `1` for a legacy v1 database that predates `_schema_meta` versioning
///   (tables exist but no version row was ever written).
/// - `N` for a versioned database, read from `_schema_meta`.
fn read_schema_version(conn: &Connection) -> SqliteResult<i32> {
    match conn.query_row(
        "SELECT value FROM _schema_meta WHERE key = 'version'",
        [],
        |row| row.get::<_, String>(0),
    ) {
        Ok(v) => v.parse().map_err(|_| {
            rusqlite::Error::InvalidParameterName(format!("Invalid schema version: {}", v))
        }),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            // No recorded version — is this a fresh DB or a legacy v1 DB?
            if sessions_table_exists(conn)? {
                Ok(1) // tables exist but unversioned → legacy v1
            } else {
                Ok(0) // nothing exists → fresh
            }
        }
        Err(e) => Err(e),
    }
}

/// Check whether the `sessions` table exists.
fn sessions_table_exists(conn: &Connection) -> SqliteResult<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'sessions'",
        [],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Create all tables and indexes at the current schema version (fresh DBs only).
fn create_all_tables(conn: &Connection) -> SqliteResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions (
            id          TEXT PRIMARY KEY,
            chat_id     TEXT,
            title       TEXT NOT NULL,
            model       TEXT NOT NULL,
            source      TEXT NOT NULL DEFAULT 'gui',
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            is_active   INTEGER DEFAULT 1
        );

        CREATE TABLE IF NOT EXISTS messages (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id   TEXT NOT NULL REFERENCES sessions(id),
            role         TEXT NOT NULL,
            content      TEXT NOT NULL,
            tool_name    TEXT,
            tool_call_id TEXT,
            tool_info    TEXT,
            tokens       INTEGER,
            created_at   TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_messages_session
            ON messages(session_id);
        CREATE INDEX IF NOT EXISTS idx_messages_created
            ON messages(session_id, created_at);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_chat_id
            ON sessions(chat_id) WHERE chat_id IS NOT NULL;

        CREATE TABLE IF NOT EXISTS memories (
            id           TEXT PRIMARY KEY,
            session_id   TEXT,
            chat_id      TEXT,
            memory_type  TEXT NOT NULL,
            title        TEXT NOT NULL,
            content      TEXT NOT NULL,
            tags         TEXT,
            is_active    INTEGER DEFAULT 1,
            created_at   TEXT NOT NULL,
            updated_at   TEXT NOT NULL,

            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE SET NULL
        );

        CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type);
        CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_memories_session ON memories(session_id);
        CREATE INDEX IF NOT EXISTS idx_memories_chat ON memories(chat_id);
        CREATE INDEX IF NOT EXISTS idx_memories_active ON memories(is_active) WHERE is_active = 1;",
    )?;
    Ok(())
}

/// Run the migration chain from `from` to `to`, one step at a time.
fn migrate(conn: &Connection, from: i32, to: i32) -> SqliteResult<()> {
    let mut current = from;
    while current < to {
        tracing::info!("Migrating database: v{} → v{}", current, current + 1);
        match current {
            1 => migrate_v1_to_v2(conn)?,
            2 => migrate_v2_to_v3(conn)?,
            other => {
                return Err(rusqlite::Error::InvalidParameterName(format!(
                    "Unknown schema version: {}",
                    other
                )))
            }
        }
        current += 1;
        write_schema_version(conn, current)?;
        tracing::info!("Database migrated to v{}", current);
    }
    Ok(())
}

/// v1 → v2: add `chat_id`, `source` to sessions; ensure `tool_info` on messages;
/// add the partial unique index on `sessions.chat_id`.
fn migrate_v1_to_v2(conn: &Connection) -> SqliteResult<()> {
    let _ = conn.execute("ALTER TABLE sessions ADD COLUMN chat_id TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE sessions ADD COLUMN source TEXT NOT NULL DEFAULT 'gui'",
        [],
    );
    let _ = conn.execute("ALTER TABLE messages ADD COLUMN tool_info TEXT", []);
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_chat_id
            ON sessions(chat_id) WHERE chat_id IS NOT NULL;",
    )?;
    Ok(())
}

/// v2 → v3: add `memories` table for long-term memory.
fn migrate_v2_to_v3(conn: &Connection) -> SqliteResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memories (
            id           TEXT PRIMARY KEY,
            session_id   TEXT,
            chat_id      TEXT,
            memory_type  TEXT NOT NULL,
            title        TEXT NOT NULL,
            content      TEXT NOT NULL,
            tags         TEXT,
            is_active    INTEGER DEFAULT 1,
            created_at   TEXT NOT NULL,
            updated_at   TEXT NOT NULL,

            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE SET NULL
        );

        CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type);
        CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_memories_session ON memories(session_id);
        CREATE INDEX IF NOT EXISTS idx_memories_chat ON memories(chat_id);
        CREATE INDEX IF NOT EXISTS idx_memories_active ON memories(is_active) WHERE is_active = 1;",
    )?;
    Ok(())
}

// ============================================================================
// Schema version helpers (private)
// ============================================================================

fn ensure_meta_table(conn: &Connection) -> SqliteResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _schema_meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
    )
}

fn write_schema_version(conn: &Connection, version: i32) -> SqliteResult<()> {
    conn.execute(
        "INSERT OR REPLACE INTO _schema_meta (key, value) VALUES ('version', ?1)",
        rusqlite::params![version.to_string()],
    )?;
    Ok(())
}

// ============================================================================
// Session and message accessors (public)
// ============================================================================

/// Insert a new session.
///
/// `chat_id` is `Some` only for Bot platforms (the platform chat identifier);
/// pass `None` for GUI/TUI sessions. `source` records which frontend created
/// the session (`"gui"`, `"tui"`, `"qq"`, `"feishu"`).
pub fn insert_session(
    conn: &Connection,
    id: &str,
    chat_id: Option<&str>,
    title: &str,
    model: &str,
    source: &str,
) -> SqliteResult<()> {
    let now = current_timestamp();
    conn.execute(
        "INSERT INTO sessions (id, chat_id, title, model, source, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, chat_id, title, model, source, now, now],
    )?;
    Ok(())
}

/// List all active sessions, ordered by most recently updated.
///
/// Pass `Some(source)` to filter by frontend (e.g. `"qq"`); `None` returns
/// sessions from all sources.
pub fn list_sessions(
    conn: &Connection,
    source_filter: Option<&str>,
) -> SqliteResult<Vec<SessionInfo>> {
    let sql = if source_filter.is_some() {
        "SELECT id, chat_id, title, model, source, created_at, updated_at \
         FROM sessions WHERE is_active = 1 AND source = ?1 ORDER BY updated_at DESC"
    } else {
        "SELECT id, chat_id, title, model, source, created_at, updated_at \
         FROM sessions WHERE is_active = 1 ORDER BY updated_at DESC"
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = if let Some(source) = source_filter {
        stmt.query_map(params![source], map_session_row)?
    } else {
        stmt.query_map([], map_session_row)?
    };
    rows.collect()
}

/// Find an active session by its platform chat identifier.
///
/// Returns `None` for GUI/TUI sessions (where `chat_id` is NULL).
pub fn find_session_by_chat_id(
    conn: &Connection,
    chat_id: &str,
) -> SqliteResult<Option<SessionInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, chat_id, title, model, source, created_at, updated_at \
         FROM sessions WHERE chat_id = ?1 AND is_active = 1",
    )?;
    let mut rows = stmt.query_map(params![chat_id], map_session_row)?;
    match rows.next() {
        Some(Ok(session)) => Ok(Some(session)),
        _ => Ok(None),
    }
}

/// List ALL sessions (including inactive ones) for a chat_id, ordered by most recent first.
pub fn list_all_sessions_by_chat_id(
    conn: &Connection,
    chat_id: &str,
) -> SqliteResult<Vec<SessionInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, chat_id, title, model, source, created_at, updated_at \
         FROM sessions WHERE chat_id = ?1 ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map(params![chat_id], map_session_row)?;
    rows.collect()
}

/// Activate a specific session and deactivate others for the same chat_id.
pub fn activate_session(
    conn: &Connection,
    session_id: &str,
    chat_id: &str,
) -> SqliteResult<()> {
    let now = current_timestamp();
    // First deactivate all sessions for this chat_id
    conn.execute(
        "UPDATE sessions SET is_active = 0 WHERE chat_id = ?1",
        params![chat_id],
    )?;
    // Then activate the target session and update its timestamp
    conn.execute(
        "UPDATE sessions SET is_active = 1, updated_at = ?1 WHERE id = ?2",
        params![now, session_id],
    )?;
    Ok(())
}

/// Get a single session by ID.
pub fn get_session(conn: &Connection, id: &str) -> SqliteResult<Option<SessionInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, chat_id, title, model, source, created_at, updated_at \
         FROM sessions WHERE id = ?1 AND is_active = 1",
    )?;
    let mut rows = stmt.query_map(params![id], map_session_row)?;
    match rows.next() {
        Some(Ok(session)) => Ok(Some(session)),
        _ => Ok(None),
    }
}

/// Row mapper shared by all session SELECT queries.
fn map_session_row(row: &rusqlite::Row<'_>) -> SqliteResult<SessionInfo> {
    Ok(SessionInfo {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        title: row.get(2)?,
        model: row.get(3)?,
        source: row.get(4)?,
        status: "idle".to_string(),
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

/// Update a session's title.
pub fn update_session_title(conn: &Connection, id: &str, title: &str) -> SqliteResult<()> {
    let now = current_timestamp();
    conn.execute(
        "UPDATE sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
        params![title, now, id],
    )?;
    Ok(())
}

/// Update a session's updated_at timestamp.
pub fn touch_session(conn: &Connection, id: &str) -> SqliteResult<()> {
    let now = current_timestamp();
    conn.execute(
        "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
        params![now, id],
    )?;
    Ok(())
}

/// Soft-delete a session.
pub fn delete_session(conn: &Connection, id: &str) -> SqliteResult<()> {
    conn.execute(
        "UPDATE sessions SET is_active = 0 WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

/// Insert a message into a session.
pub fn insert_message(
    conn: &Connection,
    session_id: &str,
    role: &str,
    content: &str,
    tool_name: Option<&str>,
    tool_call_id: Option<&str>,
    tool_info: Option<&str>,
) -> SqliteResult<i64> {
    let now = current_timestamp();
    conn.execute(
        "INSERT INTO messages (session_id, role, content, tool_name, tool_call_id, tool_info, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![session_id, role, content, tool_name, tool_call_id, tool_info, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Get all messages for a session, ordered by creation time.
pub fn get_messages(conn: &Connection, session_id: &str) -> SqliteResult<Vec<MessageData>> {
    let mut stmt = conn.prepare(
        "SELECT id, role, content, tool_name, tool_call_id, tool_info, created_at FROM messages WHERE session_id = ?1 ORDER BY id ASC"
    )?;
    let rows = stmt.query_map(params![session_id], |row| {
        let tool_info_str: Option<String> = row.get(5)?;
        let tool_info = tool_info_str.and_then(|s| serde_json::from_str(&s).ok());
        Ok(MessageData {
            id: row.get(0)?,
            role: row.get(1)?,
            content: row.get(2)?,
            tool_name: row.get(3)?,
            tool_call_id: row.get(4)?,
            tool_info,
            created_at: row.get(6)?,
        })
    })?;
    rows.collect()
}

/// Update a tool message with result.
pub fn update_tool_message(
    conn: &Connection,
    session_id: &str,
    tool_call_id: &str,
    tool_info: &str,
) -> SqliteResult<()> {
    conn.execute(
        "UPDATE messages SET tool_info = ?1 WHERE session_id = ?2 AND tool_call_id = ?3",
        params![tool_info, session_id, tool_call_id],
    )?;
    Ok(())
}

// ============================================================================
// Message conversion to/from ChatCompletionRequestMessage (new)
// ============================================================================

/// Convert stored MessageData to ChatCompletionRequestMessage
pub fn message_to_chat_message(data: &MessageData) -> Result<ChatCompletionRequestMessage> {
    match data.role.as_str() {
        "user" => Ok(ChatCompletionRequestMessage::User(
            ChatCompletionRequestUserMessage {
                content: ChatCompletionRequestUserMessageContent::Text(data.content.clone()),
                name: None,
            }
            .into(),
        )),
        "assistant" => {
            // Try to parse tool_calls from tool_info
            let tool_calls = if let Some(tool_info) = &data.tool_info {
                if let serde_json::Value::Object(obj) = tool_info {
                    // First try the "tool_calls" field (for complete tool calls)
                    if let Some(serde_json::Value::Array(arr)) = obj.get("tool_calls") {
                        use async_openai::types::chat::{
                            ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
                        };
                        let mut calls = Vec::new();
                        for call_val in arr {
                            if let Ok(call) =
                                serde_json::from_value::<ChatCompletionMessageToolCall>(
                                    call_val.clone(),
                                )
                            {
                                calls.push(ChatCompletionMessageToolCalls::Function(call));
                            }
                        }
                        if !calls.is_empty() {
                            Some(calls)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            let content = if data.content.is_empty() {
                None
            } else {
                Some(data.content.clone().into())
            };

            // Ensure assistant message has either content or tool_calls
            if content.is_none() && tool_calls.is_none() {
                // Skip this invalid message - it will cause API error
                tracing::warn!("Skipping invalid assistant message: both content and tool_calls are None (message id: {:?})", data.id);
                return Err(crate::error::AgentError::InternalError("Invalid assistant message".to_string()));
            }

            Ok(ChatCompletionRequestMessage::Assistant(
                ChatCompletionRequestAssistantMessage {
                    content,
                    name: None,
                    tool_calls,
                    refusal: None,
                    audio: None,
                    #[allow(deprecated)]
                    function_call: None,
                }
                .into(),
            ))
        }
        "tool" => {
            let tool_call_id = data.tool_call_id.clone().unwrap_or_default();
            Ok(ChatCompletionRequestMessage::Tool(
                ChatCompletionRequestToolMessage {
                    content: data.content.clone().into(),
                    tool_call_id,
                }
                .into(),
            ))
        }
        // Ignore other roles for now (system is regenerated)
        _ => Err(crate::error::AgentError::InternalError(format!(
            "Unknown role: {}",
            data.role
        ))),
    }
}

/// Load all messages for a session and convert to chat messages
pub fn load_chat_messages(
    conn: &Connection,
    session_id: &str,
) -> Result<Vec<ChatCompletionRequestMessage>> {
    let messages = get_messages(conn, session_id)?;
    tracing::debug!(
        "load_chat_messages: session_id={}, loaded {} messages from DB",
        session_id,
        messages.len()
    );

    let mut result = Vec::with_capacity(messages.len());
    for (idx, msg) in messages.iter().enumerate() {
        match message_to_chat_message(&msg) {
            Ok(chat_msg) => {
                // Double-check that assistant messages are valid before adding
                let is_valid = match &chat_msg {
                    ChatCompletionRequestMessage::Assistant(assistant_msg) => {
                        assistant_msg.content.is_some() || assistant_msg.tool_calls.is_some()
                    }
                    _ => true,
                };

                if is_valid {
                    tracing::debug!(
                        "  Message {}: role={}, content_len={}",
                        idx,
                        msg.role,
                        msg.content.len()
                    );
                    result.push(chat_msg);
                } else {
                    tracing::warn!(
                        "Skipping invalid assistant message {} (has neither content nor tool_calls)",
                        idx
                    );
                }
            }
            Err(e) => {
                tracing::warn!("Skipping invalid message {}: {}", idx, e);
            }
        }
    }
    tracing::debug!("load_chat_messages: successfully converted {} messages", result.len());
    Ok(result)
}

// ============================================================================
// Memory accessors (public)
// ============================================================================

/// Insert a new memory into the database.
pub fn insert_memory(conn: &Connection, memory: &Memory) -> SqliteResult<()> {
    let tags_str = if memory.tags.is_empty() {
        None
    } else {
        Some(memory.tags.join(","))
    };

    conn.execute(
        "INSERT INTO memories (
            id, session_id, chat_id, memory_type, title, content, tags, is_active, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            memory.id,
            memory.session_id,
            memory.chat_id,
            memory.memory_type.as_str(),
            memory.title,
            memory.content,
            tags_str,
            memory.is_active,
            memory.created_at,
            memory.updated_at,
        ],
    )?;
    Ok(())
}

/// Update an existing memory.
pub fn update_memory(conn: &Connection, memory: &Memory) -> SqliteResult<()> {
    let tags_str = if memory.tags.is_empty() {
        None
    } else {
        Some(memory.tags.join(","))
    };

    conn.execute(
        "UPDATE memories SET
            title = ?1,
            content = ?2,
            tags = ?3,
            memory_type = ?4,
            updated_at = ?5
        WHERE id = ?6",
        params![
            memory.title,
            memory.content,
            tags_str,
            memory.memory_type.as_str(),
            current_timestamp(),
            memory.id,
        ],
    )?;
    Ok(())
}

/// Deactivate a memory (soft delete).
pub fn deactivate_memory(conn: &Connection, memory_id: &str) -> SqliteResult<()> {
    conn.execute(
        "UPDATE memories SET is_active = 0, updated_at = ?1 WHERE id = ?2",
        params![current_timestamp(), memory_id],
    )?;
    Ok(())
}

/// Permanently delete a memory (hard delete).
pub fn delete_memory_permanently(conn: &Connection, memory_id: &str) -> SqliteResult<()> {
    conn.execute("DELETE FROM memories WHERE id = ?1", params![memory_id])?;
    Ok(())
}

/// Get a single memory by ID.
pub fn get_memory(conn: &Connection, memory_id: &str) -> SqliteResult<Option<Memory>> {
    let mut stmt = conn.prepare(
        "SELECT id, session_id, chat_id, memory_type, title, content, tags, is_active, created_at, updated_at
         FROM memories WHERE id = ?1",
    )?;

    let mut rows = stmt.query_map(params![memory_id], map_memory_row)?;
    rows.next().transpose()
}

/// Find memories by title (fuzzy match using LIKE).
pub fn find_memories_by_title(
    conn: &Connection,
    title_part: &str,
    filter: &MemoryFilter,
    limit: Option<usize>,
) -> SqliteResult<Vec<Memory>> {
    let (sql, params) = build_memory_query(Some(title_part), filter, limit);
    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt.query_map(rusqlite::params_from_iter(params), map_memory_row)?;
    rows.collect()
}

/// List memories with optional filtering.
pub fn list_memories(
    conn: &Connection,
    filter: &MemoryFilter,
    limit: Option<usize>,
) -> SqliteResult<Vec<Memory>> {
    let (sql, params) = build_memory_query(None, filter, limit);
    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt.query_map(rusqlite::params_from_iter(params), map_memory_row)?;
    rows.collect()
}

/// Recall memories using keyword search (title, content, tags).
pub fn recall_memories(
    conn: &Connection,
    query: &str,
    filter: &MemoryFilter,
    limit: usize,
) -> SqliteResult<Vec<Memory>> {
    // First try exact match in title
    let mut results = find_memories_by_title(conn, query, filter, Some(limit))?;

    // If we have enough results, return them
    if results.len() >= limit {
        results.truncate(limit);
        return Ok(results);
    }

    // Otherwise, do a broader search using LIKE on title or content
    let remaining = limit - results.len();
    let (sql, params) = build_recall_query(query, filter, remaining);
    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt.query_map(rusqlite::params_from_iter(params), map_memory_row)?;
    for row in rows {
        let memory = row?;
        if !results.iter().any(|m| m.id == memory.id) {
            results.push(memory);
        }
    }

    results.truncate(limit);
    Ok(results)
}

// ============================================================================
// Memory helpers (private)
// ============================================================================

fn map_memory_row(row: &rusqlite::Row) -> SqliteResult<Memory> {
    let tags_str: Option<String> = row.get(6)?;
    let tags = tags_str
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();

    Ok(Memory {
        id: row.get(0)?,
        session_id: row.get(1)?,
        chat_id: row.get(2)?,
        memory_type: MemoryType::from_str(&row.get::<_, String>(3)?),
        title: row.get(4)?,
        content: row.get(5)?,
        tags,
        is_active: row.get::<_, i32>(7)? != 0,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn build_memory_query<'a>(
    title_search: Option<&'a str>,
    filter: &'a MemoryFilter,
    limit: Option<usize>,
) -> (String, Vec<rusqlite::types::ToSqlOutput<'a>>) {
    let mut conditions = Vec::new();
    let mut params: Vec<rusqlite::types::ToSqlOutput> = Vec::new();

    // Active flag
    if filter.only_active {
        conditions.push("is_active = 1".to_string());
    }

    // Type filter
    if let Some(memory_type) = &filter.memory_type {
        conditions.push("memory_type = ?".to_string());
        params.push(memory_type.as_str().into());
    }

    // Session ID
    if let Some(session_id) = &filter.session_id {
        conditions.push("session_id = ?".to_string());
        params.push(session_id.as_str().into());
    }

    // Chat ID
    if let Some(chat_id) = &filter.chat_id {
        conditions.push("chat_id = ?".to_string());
        params.push(chat_id.as_str().into());
    }

    // Since time
    if let Some(since) = &filter.since {
        conditions.push("created_at >= ?".to_string());
        params.push(since.as_str().into());
    }

    // Title search
    if let Some(title) = title_search {
        conditions.push("title LIKE ?".to_string());
        params.push(format!("%{}%", title).into());
    }

    // Tags filter (any match)
    if let Some(tags) = &filter.tags {
        if !tags.is_empty() {
            let tag_conditions: Vec<_> = tags.iter().map(|_| "tags LIKE ?").collect();
            conditions.push(format!("({})", tag_conditions.join(" OR ")));
            for tag in tags {
                params.push(format!("%{}%", tag).into());
            }
        }
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let limit_clause = limit.map(|l| format!("LIMIT {}", l)).unwrap_or_default();

    let sql = format!(
        "SELECT id, session_id, chat_id, memory_type, title, content, tags, is_active, created_at, updated_at
         FROM memories
         {}
         ORDER BY created_at DESC
         {}",
        where_clause, limit_clause
    );

    (sql, params)
}

fn build_recall_query<'a>(
    query: &'a str,
    filter: &'a MemoryFilter,
    limit: usize,
) -> (String, Vec<rusqlite::types::ToSqlOutput<'a>>) {
    let mut conditions = Vec::new();
    let mut params: Vec<rusqlite::types::ToSqlOutput> = Vec::new();

    // Active flag
    if filter.only_active {
        conditions.push("is_active = 1".to_string());
    }

    // Type filter
    if let Some(memory_type) = &filter.memory_type {
        conditions.push("memory_type = ?".to_string());
        params.push(memory_type.as_str().into());
    }

    // Session ID
    if let Some(session_id) = &filter.session_id {
        conditions.push("session_id = ?".to_string());
        params.push(session_id.as_str().into());
    }

    // Chat ID
    if let Some(chat_id) = &filter.chat_id {
        conditions.push("chat_id = ?".to_string());
        params.push(chat_id.as_str().into());
    }

    // Keyword search (match in title, content, or tags)
    conditions.push("(title LIKE ? OR content LIKE ? OR tags LIKE ?)".to_string());
    let pattern = format!("%{}%", query);
    params.push(pattern.clone().into());
    params.push(pattern.clone().into());
    params.push(pattern.into());

    let where_clause = format!("WHERE {}", conditions.join(" AND "));

    let sql = format!(
        "SELECT id, session_id, chat_id, memory_type, title, content, tags, is_active, created_at, updated_at
         FROM memories
         {}
         ORDER BY created_at DESC
         LIMIT {}",
        where_clause, limit
    );

    (sql, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_local_db_path() {
        let working_dir = PathBuf::from("project");
        let path = resolve_db_path(&working_dir, false).unwrap();
        assert_eq!(
            path,
            working_dir.join(ROBIT_DIR).join(MEMORY_DIR).join(DB_FILE)
        );
    }

    #[test]
    fn session_crud() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        insert_session(
            &conn,
            "test-123",
            None,
            "Test Session",
            "deepseek/deepseek-chat",
            "gui",
        )
        .unwrap();

        let sessions = list_sessions(&conn, None).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "test-123");
        assert_eq!(sessions[0].title, "Test Session");
        assert_eq!(sessions[0].source, "gui");
        assert_eq!(sessions[0].chat_id, None);
        assert_eq!(sessions[0].status, "idle");

        let session = get_session(&conn, "test-123").unwrap().unwrap();
        assert_eq!(session.title, "Test Session");
        assert_eq!(session.source, "gui");

        update_session_title(&conn, "test-123", "Updated Title").unwrap();
        let updated = get_session(&conn, "test-123").unwrap().unwrap();
        assert_eq!(updated.title, "Updated Title");

        delete_session(&conn, "test-123").unwrap();
        assert!(get_session(&conn, "test-123").unwrap().is_none());
        assert!(list_sessions(&conn, None).unwrap().is_empty());
    }

    #[test]
    fn message_operations() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        insert_session(&conn, "session-msg", None, "Chat Session", "model", "gui").unwrap();
        let user_id = insert_message(
            &conn,
            "session-msg",
            "user",
            "Hello Robit",
            None,
            None,
            None,
        )
        .unwrap();
        let assistant_id = insert_message(
            &conn,
            "session-msg",
            "assistant",
            "Hello! How can I help?",
            None,
            None,
            None,
        )
        .unwrap();

        let messages = get_messages(&conn, "session-msg").unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].id, user_id);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "Hello Robit");
        assert_eq!(messages[1].id, assistant_id);
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].content, "Hello! How can I help?");
    }

    #[test]
    fn empty_sessions() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let sessions = list_sessions(&conn, None).unwrap();
        assert_eq!(sessions.len(), 0);
    }

    #[test]
    fn get_nonexistent_session() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let session = get_session(&conn, "nonexistent").unwrap();
        assert!(session.is_none());
    }

    #[test]
    fn tool_message_update() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        insert_session(&conn, "session-tool", None, "Tool Session", "model", "gui").unwrap();
        let initial = serde_json::json!({
            "tool_call_id": "tool-1",
            "name": "bash",
            "arguments": "{}",
            "status": "pending",
            "requires_confirm": true
        })
        .to_string();
        insert_message(
            &conn,
            "session-tool",
            "tool",
            "{}",
            Some("bash"),
            Some("tool-1"),
            Some(&initial),
        )
        .unwrap();

        let updated = serde_json::json!({
            "tool_call_id": "tool-1",
            "status": "success",
            "output": "done"
        })
        .to_string();
        update_tool_message(&conn, "session-tool", "tool-1", &updated).unwrap();

        let messages = get_messages(&conn, "session-tool").unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tool_name.as_deref(), Some("bash"));
        assert_eq!(messages[0].tool_call_id.as_deref(), Some("tool-1"));
        assert_eq!(messages[0].tool_info.as_ref().unwrap()["status"], "success");
        assert_eq!(messages[0].tool_info.as_ref().unwrap()["output"], "done");
    }

    #[test]
    fn chat_id_lookup_and_source_filter() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        insert_session(&conn, "gui-1", None, "GUI Session", "model", "gui").unwrap();
        insert_session(
            &conn,
            "qq-1",
            Some("group:abc"),
            "技术讨论群",
            "model",
            "qq",
        )
        .unwrap();
        insert_session(
            &conn,
            "qq-2",
            Some("private:xyz"),
            "私聊",
            "model",
            "qq",
        )
        .unwrap();

        // find_session_by_chat_id
        let found = find_session_by_chat_id(&conn, "group:abc").unwrap().unwrap();
        assert_eq!(found.id, "qq-1");
        assert_eq!(found.source, "qq");
        assert_eq!(found.chat_id.as_deref(), Some("group:abc"));

        // chat_id lookup returns None for GUI sessions (NULL chat_id)
        assert!(find_session_by_chat_id(&conn, "does-not-exist")
            .unwrap()
            .is_none());

        // source filter
        let qq_sessions = list_sessions(&conn, Some("qq")).unwrap();
        assert_eq!(qq_sessions.len(), 2);
        assert!(qq_sessions.iter().all(|s| s.source == "qq"));

        let gui_sessions = list_sessions(&conn, Some("gui")).unwrap();
        assert_eq!(gui_sessions.len(), 1);
        assert_eq!(gui_sessions[0].id, "gui-1");

        // no filter returns all
        assert_eq!(list_sessions(&conn, None).unwrap().len(), 3);
    }

    #[test]
    fn chat_id_unique_per_chat() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        insert_session(
            &conn,
            "qq-1",
            Some("group:abc"),
            "First",
            "model",
            "qq",
        )
        .unwrap();
        // Inserting a second session with the same chat_id must fail (unique index).
        let err = insert_session(&conn, "qq-2", Some("group:abc"), "Second", "model", "qq");
        assert!(err.is_err());
    }

    #[test]
    fn migrates_legacy_v1_database() {
        let conn = Connection::open_in_memory().unwrap();
        // Simulate a legacy v1 database: old schema, no _schema_meta.
        conn.execute_batch(
            "CREATE TABLE sessions (
                id          TEXT PRIMARY KEY,
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
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, title, model, created_at, updated_at) \
             VALUES ('legacy-1', 'Legacy', 'model', '2020-01-01', '2020-01-01')",
            [],
        )
        .unwrap();

        // Run init_db — it should detect v0 → ... actually no _schema_meta means version 0,
        // but tables already exist. create_all_tables uses IF NOT EXISTS so it's safe,
        // and version is written as current. The legacy row's source defaults to 'gui'.
        init_db(&conn).unwrap();

        // Schema version is now current.
        let v: i32 = read_schema_version(&conn).unwrap();
        assert_eq!(v, CURRENT_SCHEMA_VERSION);

        // New columns exist and legacy data is preserved.
        let session = get_session(&conn, "legacy-1").unwrap().unwrap();
        assert_eq!(session.title, "Legacy");
        assert_eq!(session.source, "gui");
        assert_eq!(session.chat_id, None);
    }

    #[test]
    fn init_db_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        // Running again on an already-current DB must not error.
        init_db(&conn).unwrap();
        assert_eq!(read_schema_version(&conn).unwrap(), CURRENT_SCHEMA_VERSION);
    }
}
