use robit_agent::event::{FrontendMessage, new_session_id};
use tauri::{AppHandle, State};

use crate::db;
use crate::events::{ConfigInfo, MessageData, SessionInfo};
use crate::state::AppState;

/// Create a new session and its Agent.
#[tauri::command]
pub async fn create_session(
    app_handle: AppHandle,
    state: State<'_, AppState>,
    model: String,
) -> Result<SessionInfo, String> {
    let session_id = new_session_id();
    let title = "New Session".to_string();

    // Insert into DB
    {
        let db = state.db.lock().await;
        db::insert_session(&db, &session_id, &title, &model)
            .map_err(|e| format!("DB error: {}", e))?;
    }

    // Spawn agent
    let handle = state.spawn_agent(&session_id, &app_handle).await?;

    // Register in agents map
    {
        let mut agents = state.agents.lock().await;
        agents.insert(session_id.clone(), handle);
    }

    // Set as active session
    {
        let mut active = state.active_session.lock().await;
        *active = Some(session_id.clone());
    }

    Ok(SessionInfo {
        id: session_id,
        title,
        model,
        status: "ready".to_string(),
        created_at: String::new(),
        updated_at: String::new(),
    })
}

/// List all active sessions.
#[tauri::command]
pub async fn list_sessions(
    state: State<'_, AppState>,
) -> Result<Vec<SessionInfo>, String> {
    state.session_list().await
}

/// Switch to a different session and load its message history.
#[tauri::command]
pub async fn switch_session(
    app_handle: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<MessageData>, String> {
    // Ensure Agent exists for this session (create if not)
    let needs_agent = {
        let agents = state.agents.lock().await;
        !agents.contains_key(&session_id)
    };

    if needs_agent {
        let handle = state.spawn_agent(&session_id, &app_handle).await?;
        let mut agents = state.agents.lock().await;
        agents.insert(session_id.clone(), handle);
    }

    // Set as active session
    {
        let mut active = state.active_session.lock().await;
        *active = Some(session_id.clone());
    }

    // Load messages from DB
    let db = state.db.lock().await;
    db::get_messages(&db, &session_id).map_err(|e| format!("DB error: {}", e))
}

/// Send a user message to the active session's Agent.
#[tauri::command]
pub async fn send_message(
    state: State<'_, AppState>,
    session_id: String,
    content: String,
) -> Result<(), String> {
    // Save user message to DB
    {
        let db = state.db.lock().await;
        db::insert_message(&db, &session_id, "user", &content, None, None)
            .map_err(|e| format!("DB error: {}", e))?;
        db::touch_session(&db, &session_id).map_err(|e| format!("DB error: {}", e))?;
    }

    // Send to Agent
    let agents = state.agents.lock().await;
    let handle = agents
        .get(&session_id)
        .ok_or_else(|| format!("Session {} not found", session_id))?;

    handle
        .message_tx
        .send(FrontendMessage::UserInput(content))
        .await
        .map_err(|e| format!("Failed to send message: {}", e))?;

    Ok(())
}

/// Cancel the running Agent in a session.
#[tauri::command]
pub async fn cancel(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), String> {
    let agents = state.agents.lock().await;
    if let Some(handle) = agents.get(&session_id) {
        handle.cancel_token.cancel();
    }
    Ok(())
}

/// Soft-delete a session.
#[tauri::command]
pub async fn delete_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), String> {
    // Cancel agent if running
    {
        let agents = state.agents.lock().await;
        if let Some(handle) = agents.get(&session_id) {
            handle.cancel_token.cancel();
        }
    }

    // Remove from agents map
    {
        let mut agents = state.agents.lock().await;
        agents.remove(&session_id);
    }

    // Soft-delete in DB
    {
        let db = state.db.lock().await;
        db::delete_session(&db, &session_id).map_err(|e| format!("DB error: {}", e))?;
    }

    // If this was the active session, switch to the nearest one
    {
        let mut active = state.active_session.lock().await;
        if active.as_deref() == Some(&session_id) {
            let db = state.db.lock().await;
            let sessions = db::list_sessions(&db).map_err(|e| format!("DB error: {}", e))?;
            *active = sessions.first().map(|s| s.id.clone());
        }
    }

    Ok(())
}

/// Rename a session.
#[tauri::command]
pub async fn rename_session(
    state: State<'_, AppState>,
    session_id: String,
    title: String,
) -> Result<(), String> {
    let db = state.db.lock().await;
    db::update_session_title(&db, &session_id, &title)
        .map_err(|e| format!("DB error: {}", e))
}

/// Get messages for a session.
#[tauri::command]
pub async fn get_messages(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<MessageData>, String> {
    let db = state.db.lock().await;
    db::get_messages(&db, &session_id).map_err(|e| format!("DB error: {}", e))
}

/// Respond to a tool confirmation request.
#[tauri::command]
pub async fn confirm_tool(
    state: State<'_, AppState>,
    session_id: String,
    tool_call_id: String,
    approved: bool,
) -> Result<(), String> {
    let key = format!("{}:{}", session_id, tool_call_id);
    let mut map = state.confirmations.lock().await;
    if let Some(tx) = map.remove(&key) {
        let _ = tx.send(approved);
    }
    Ok(())
}

/// Get non-sensitive configuration for the frontend.
#[tauri::command]
pub async fn get_config(
    state: State<'_, AppState>,
) -> Result<ConfigInfo, String> {
    Ok(state.config_info())
}
