use rusqlite::{Connection, Result as SqliteResult, params};

use crate::events::{MessageData, SessionInfo};

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
        "
    )?;

    // Try to add tool_info column, ignore error if already exists
    let _ = conn.execute("ALTER TABLE messages ADD COLUMN tool_info TEXT", ());

    Ok(())
}

/// Insert a new session.
pub fn insert_session(conn: &Connection, id: &str, title: &str, model: &str) -> SqliteResult<()> {
    let now = chrono_now();
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
    let now = chrono_now();
    conn.execute(
        "UPDATE sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
        params![title, now, id],
    )?;
    Ok(())
}

/// Update a session's updated_at timestamp.
pub fn touch_session(conn: &Connection, id: &str) -> SqliteResult<()> {
    let now = chrono_now();
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
    let now = chrono_now();
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

/// Get an ISO 8601 timestamp string without chrono dependency.
fn chrono_now() -> String {
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();

    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    let (year, month, day) = days_to_date(days as i64);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        year, month, day, hours, minutes, seconds
    )
}

fn days_to_date(mut days: i64) -> (i64, u32, u32) {
    days += 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { (mp + 3) as u32 } else { (mp - 9) as u32 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}
