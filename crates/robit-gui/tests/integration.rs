//! Integration tests for robit-gui.
//! Tests core DB operations and data structures.

use rusqlite::Connection;
use robit_gui::db::{init_db, insert_session, list_sessions, get_session, update_session_title, delete_session, insert_message, get_messages};
use robit_gui::events::{SessionInfo, MessageData};
use std::collections::HashMap;

/// Test DB initialization and session CRUD operations.
#[test]
fn test_session_crud() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();

    // Insert test session
    insert_session(&conn, "test-123", "Test Session", "deepseek/deepseek-chat").unwrap();

    // List sessions
    let sessions = list_sessions(&conn).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "test-123");
    assert_eq!(sessions[0].title, "Test Session");

    // Get single session
    let session = get_session(&conn, "test-123").unwrap();
    assert!(session.is_some());
    let session = session.unwrap();
    assert_eq!(session.title, "Test Session");

    // Update title
    update_session_title(&conn, "test-123", "Updated Title").unwrap();
    let updated = get_session(&conn, "test-123").unwrap().unwrap();
    assert_eq!(updated.title, "Updated Title");

    // Soft delete
    delete_session(&conn, "test-123").unwrap();
    let sessions_after_delete = list_sessions(&conn).unwrap();
    assert_eq!(sessions_after_delete.len(), 0);
}

/// Test message insert and retrieval.
#[test]
fn test_message_operations() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();

    insert_session(&conn, "session-msg", "Chat Session", "model").unwrap();

    // Insert messages
    insert_message(&conn, "session-msg", "user", "Hello Robit", None, None).unwrap();
    insert_message(&conn, "session-msg", "assistant", "Hello! How can I help?", None, None).unwrap();

    // Get messages
    let messages = get_messages(&conn, "session-msg").unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "Hello Robit");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].content, "Hello! How can I help?");
}

/// Test empty session list.
#[test]
fn test_empty_sessions() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();

    let sessions = list_sessions(&conn).unwrap();
    assert_eq!(sessions.len(), 0);
}

/// Test getting a non-existent session.
#[test]
fn test_get_nonexistent_session() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();

    let session = get_session(&conn, "nonexistent").unwrap();
    assert!(session.is_none());
}

/// Test UiEvent serialization (JSON round trip).
#[test]
fn test_event_serialization() {
    use robit_gui::events::UiEvent;
    use serde_json;

    let event = UiEvent::TextDelta {
        session_id: "s1".to_string(),
        delta: "Hello world".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"type\":\"TextDelta\""));
    assert!(json.contains("\"session_id\":\"s1\""));
    assert!(json.contains("\"delta\":\"Hello world\""));

    let event2 = UiEvent::ToolCallRequested {
        session_id: "s1".to_string(),
        tool_call_id: "tc123".to_string(),
        name: "bash".to_string(),
        arguments: "{}".to_string(),
        requires_confirm: true,
    };
    let json2 = serde_json::to_string(&event2).unwrap();
    assert!(json2.contains("ToolCallRequested"));
    assert!(json2.contains("\"name\":\"bash\""));
}

/// Test SessionInfo serialization.
#[test]
fn test_session_info_serialization() {
    use robit_gui::events::SessionInfo;
    use serde_json;

    let info = SessionInfo {
        id: "test-id".to_string(),
        title: "Test".to_string(),
        model: "deepseek-chat".to_string(),
        status: "ready".to_string(),
        created_at: "2024-01-01T00:00:00".to_string(),
        updated_at: "2024-01-01T00:00:00".to_string(),
    };

    let json = serde_json::to_string(&info).unwrap();
    assert!(json.contains("test-id"));
    assert!(json.contains("Test"));
}

/// Test MessageData serialization.
#[test]
fn test_message_data_serialization() {
    use robit_gui::events::MessageData;
    use serde_json;

    let msg = MessageData {
        id: 1,
        role: "user".to_string(),
        content: "test message".to_string(),
        tool_name: None,
        tool_call_id: None,
        created_at: "2024-01-01".to_string(),
    };

    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("user"));
    assert!(json.contains("test message"));
}
