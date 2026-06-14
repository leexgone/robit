//! Session and message storage helpers.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, Result as SqliteResult};
use serde::{Deserialize, Serialize};

use crate::datetime::current_timestamp;

const ROBIT_DIR: &str = ".robit";
const MEMORY_DIR: &str = "memory";
const DB_FILE: &str = "robit.db";

/// Resolve the session database path for a working directory and storage scope.
pub fn resolve_db_path(working_dir: &Path, global_storage: bool) -> Result<PathBuf, String> {
    if global_storage {
        let home = dirs::home_dir().ok_or_else(|| "Cannot determine home directory".to_string())?;
        Ok(home.join(ROBIT_DIR).join(MEMORY_DIR).join(DB_FILE))
    } else {
        Ok(working_dir.join(ROBIT_DIR).join(MEMORY_DIR).join(DB_FILE))
    }
}

/// Session metadata returned to frontends.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub model: String,
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

/// Initialize the database: create tables and indexes if they don't exist.
pub fn init_db(conn: &Connection) -> SqliteResult<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS sessions (
            id          TEXT PRIMARY KEY,
            title       TEXT NOT NULL,
            model       TEXT NOT NULL,
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
            tokens       INTEGER,
            created_at   TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_messages_session
            ON messages(session_id);
        CREATE INDEX IF NOT EXISTS idx_messages_created
            ON messages(session_id, created_at);
        ",
    )?;

    // Try to add tool_info column, ignore error if already exists
    let _ = conn.execute("ALTER TABLE messages ADD COLUMN tool_info TEXT", ());

    Ok(())
}

/// Insert a new session.
pub fn insert_session(conn: &Connection, id: &str, title: &str, model: &str) -> SqliteResult<()> {
    let now = current_timestamp();
    conn.execute(
        "INSERT INTO sessions (id, title, model, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, title, model, now, now],
    )?;
    Ok(())
}

/// List all active sessions, ordered by most recently updated.
pub fn list_sessions(conn: &Connection) -> SqliteResult<Vec<SessionInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, model, created_at, updated_at FROM sessions WHERE is_active = 1 ORDER BY updated_at DESC"
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(SessionInfo {
            id: row.get(0)?,
            title: row.get(1)?,
            model: row.get(2)?,
            status: "idle".to_string(),
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
        })
    })?;
    rows.collect()
}

/// Get a single session by ID.
pub fn get_session(conn: &Connection, id: &str) -> SqliteResult<Option<SessionInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, model, created_at, updated_at FROM sessions WHERE id = ?1 AND is_active = 1"
    )?;
    let mut rows = stmt.query_map(params![id], |row| {
        Ok(SessionInfo {
            id: row.get(0)?,
            title: row.get(1)?,
            model: row.get(2)?,
            status: "idle".to_string(),
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
        })
    })?;
    match rows.next() {
        Some(Ok(session)) => Ok(Some(session)),
        _ => Ok(None),
    }
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

/// Update a tool message with output and status.
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
    fn resolves_global_db_path() {
        let path = resolve_db_path(&PathBuf::from("project"), true).unwrap();
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(DB_FILE)
        );
        assert!(path.ends_with(PathBuf::from(ROBIT_DIR).join(MEMORY_DIR).join(DB_FILE)));
    }

    #[test]
    fn session_crud() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        insert_session(&conn, "test-123", "Test Session", "deepseek/deepseek-chat").unwrap();

        let sessions = list_sessions(&conn).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "test-123");
        assert_eq!(sessions[0].title, "Test Session");
        assert_eq!(sessions[0].status, "idle");

        let session = get_session(&conn, "test-123").unwrap().unwrap();
        assert_eq!(session.title, "Test Session");

        update_session_title(&conn, "test-123", "Updated Title").unwrap();
        let updated = get_session(&conn, "test-123").unwrap().unwrap();
        assert_eq!(updated.title, "Updated Title");

        delete_session(&conn, "test-123").unwrap();
        assert!(get_session(&conn, "test-123").unwrap().is_none());
        assert!(list_sessions(&conn).unwrap().is_empty());
    }

    #[test]
    fn message_operations() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        insert_session(&conn, "session-msg", "Chat Session", "model").unwrap();
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

        let sessions = list_sessions(&conn).unwrap();
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

        insert_session(&conn, "session-tool", "Tool Session", "model").unwrap();
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
}
