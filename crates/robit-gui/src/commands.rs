use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use robit_agent::event::{FrontendMessage, new_session_id};
use tauri::{AppHandle, Emitter, State};

use crate::db;
use crate::events::{ConfigInfo, MessageData, ScheduleRunDto, ScheduleTaskDto, SessionInfo, UiEvent};
use crate::state::AppState;

/// Generate a session title from user message content.
/// Takes the first line and truncates to ~30 chars.
fn generate_session_title(content: &str) -> String {
    let first_line = content.lines().next().unwrap_or("New Session");
    let trimmed = first_line.trim();
    if trimmed.is_empty() {
        return "New Session".to_string();
    }
    if trimmed.chars().count() <= 30 {
        trimmed.to_string()
    } else {
        let prefix: String = trimmed.chars().take(28).collect();
        format!("{}...", prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::generate_session_title;

    #[test]
    fn generates_title_from_multibyte_text() {
        let title = generate_session_title("这是一个很长的中文标题，需要安全截断，不能切到半个字符，否则会导致 IPC 失败");
        assert!(title.ends_with("..."));
        assert!(title.is_char_boundary(title.len()));
    }
}

/// Create a new session and its Agent.
#[tauri::command]
pub async fn create_session(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
    model: String,
) -> Result<SessionInfo, String> {
    let session_id = new_session_id();
    let title = "New Session".to_string();

    // Insert into DB
    {
        let db = state.db.lock().await;
        db::insert_session(&db, &session_id, None, &title, &model, "gui", "")
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
        chat_id: None,
        title,
        model,
        source: "gui".to_string(),
        status: "ready".to_string(),
        created_at: String::new(),
        updated_at: String::new(),
    })
}

/// List all active sessions.
#[tauri::command]
pub async fn list_sessions(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<SessionInfo>, String> {
    state.session_list().await
}

/// Switch to a different session and load its message history.
#[tauri::command]
pub async fn switch_session(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
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
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    content: String,
) -> Result<(), String> {
    // Check if this is a new session (only "New Session" title and no messages yet)
    let should_auto_rename = {
        let db = state.db.lock().await;
        if let Ok(Some(session)) = db::get_session(&db, &session_id) {
            session.title == "New Session"
        } else {
            false
        }
    };

    // Auto-rename session based on first user message
    if should_auto_rename {
        let new_title = generate_session_title(&content);
        let db = state.db.lock().await;
        if db::update_session_title(&db, &session_id, &new_title).is_ok() {
            let _ = app_handle.emit("agent-event", UiEvent::SessionRenamed {
                session_id: session_id.clone(),
                title: new_title,
            });
        }
    }

    // Save user message to DB
    {
        let db = state.db.lock().await;
        db::insert_message(&db, &session_id, "user", &content, None, None, None, None)
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
        .send(FrontendMessage::UserInput { text: content, attachments: vec![] })
        .await
        .map_err(|e| format!("Failed to send message: {}", e))?;

    Ok(())
}

/// Cancel the running Agent in a session.
#[tauri::command]
pub async fn cancel(
    state: State<'_, Arc<AppState>>,
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
    state: State<'_, Arc<AppState>>,
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
            let sessions = db::list_sessions(&db, None).map_err(|e| format!("DB error: {}", e))?;
            *active = sessions.first().map(|s| s.id.clone());
        }
    }

    Ok(())
}

/// Rename a session.
#[tauri::command]
pub async fn rename_session(
    state: State<'_, Arc<AppState>>,
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
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Vec<MessageData>, String> {
    let db = state.db.lock().await;
    db::get_messages(&db, &session_id).map_err(|e| format!("DB error: {}", e))
}

/// Respond to a tool confirmation request.
#[tauri::command]
pub async fn confirm_tool(
    state: State<'_, Arc<AppState>>,
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
    state: State<'_, Arc<AppState>>,
) -> Result<ConfigInfo, String> {
    Ok(state.config_info())
}

/// Read a local image file and return it as a base64 data URL.
/// `image_path` is the relative path from the working directory (e.g. "images/foo.png").
#[tauri::command]
pub async fn read_image_file(
    state: State<'_, Arc<AppState>>,
    image_path: String,
) -> Result<String, String> {
    // Reject path traversal attempts
    if image_path.contains("..") {
        return Err("Invalid image path".to_string());
    }

    let abs_path: PathBuf = state.working_dir.join(&image_path);
    let bytes =
        tokio::fs::read(&abs_path)
            .await
            .map_err(|e| format!("Failed to read image: {}", e))?;

    let mime = match abs_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        _ => "image/png",
    };

    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{};base64,{}", mime, encoded))
}

// ============================================================================
// Schedule management commands (M3: GUI panel)
// ============================================================================

fn task_to_dto(t: &robit_agent::schedule::ScheduledTask) -> ScheduleTaskDto {
    ScheduleTaskDto {
        id: t.id.clone(),
        title: t.title.clone(),
        prompt: t.prompt.clone(),
        schedule: serde_json::to_value(&t.schedule).unwrap_or(serde_json::Value::Null),
        timezone: t.timezone.clone(),
        status: t.status.as_str().to_string(),
        max_runs: t.max_runs,
        run_count: t.run_count,
        next_run_at: t.next_run_at.map(robit_agent::schedule::fmt_ts),
        last_run_at: t.last_run_at.map(robit_agent::schedule::fmt_ts),
        created_at: robit_agent::schedule::fmt_ts(t.created_at),
        updated_at: robit_agent::schedule::fmt_ts(t.updated_at),
    }
}

fn run_to_dto(r: &robit_agent::schedule::RunRecord) -> ScheduleRunDto {
    ScheduleRunDto {
        id: r.id,
        scheduled_at: robit_agent::schedule::fmt_ts(r.scheduled_at),
        fired_at: robit_agent::schedule::fmt_ts(r.fired_at),
        catch_up: r.catch_up,
        status: r.status.clone(),
        error: r.error.clone(),
    }
}

/// List all scheduled tasks across all sessions (GUI is single-user, no scope filter needed).
#[tauri::command]
pub async fn list_scheduled_tasks(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<ScheduleTaskDto>, String> {
    let shared = state
        .schedule
        .as_ref()
        .ok_or_else(|| "Scheduled tasks are not enabled".to_string())?;
    // GUI: list all tasks for source="gui" regardless of session_id.
    let scope = robit_agent::schedule::TaskScope {
        source: "gui".to_string(),
        namespace: String::new(),
        chat_id: None, // None = no chat filter → all GUI tasks
    };
    let tasks = shared
        .store
        .list_scoped(&scope, None, 100, 0)
        .map_err(|e| format!("Store error: {}", e))?;
    Ok(tasks.iter().map(task_to_dto).collect())
}

/// Get one scheduled task's full details + recent runs.
#[tauri::command]
pub async fn get_scheduled_task(
    state: State<'_, Arc<AppState>>,
    task_id: String,
) -> Result<(ScheduleTaskDto, Vec<ScheduleRunDto>), String> {
    let shared = state
        .schedule
        .as_ref()
        .ok_or_else(|| "Scheduled tasks are not enabled".to_string())?;
    let scope = robit_agent::schedule::TaskScope {
        source: "gui".to_string(),
        namespace: String::new(),
        chat_id: None,
    };
    let task = shared
        .store
        .get_scoped(&task_id, &scope)
        .map_err(|e| format!("Store error: {}", e))?
        .ok_or_else(|| format!("Task '{}' not found", task_id))?;
    if task.status == robit_agent::schedule::TaskStatus::Deleted {
        return Err(format!("Task '{}' is deleted", task_id));
    }
    let runs = shared
        .store
        .runs_for(&task_id, 20)
        .map_err(|e| format!("Store error: {}", e))?;
    Ok((task_to_dto(&task), runs.iter().map(run_to_dto).collect()))
}

/// Pause a scheduled task.
#[tauri::command]
pub async fn pause_scheduled_task(
    state: State<'_, Arc<AppState>>,
    task_id: String,
) -> Result<(), String> {
    apply_schedule_action(state, &task_id, "pause").await
}

/// Resume a paused scheduled task.
#[tauri::command]
pub async fn resume_scheduled_task(
    state: State<'_, Arc<AppState>>,
    task_id: String,
) -> Result<(), String> {
    apply_schedule_action(state, &task_id, "resume").await
}

/// Soft-delete a scheduled task.
#[tauri::command]
pub async fn delete_scheduled_task(
    state: State<'_, Arc<AppState>>,
    task_id: String,
) -> Result<(), String> {
    apply_schedule_action(state, &task_id, "delete").await
}

async fn apply_schedule_action(
    state: State<'_, Arc<AppState>>,
    task_id: &str,
    action: &str,
) -> Result<(), String> {
    let shared = state
        .schedule
        .as_ref()
        .ok_or_else(|| "Scheduled tasks are not enabled".to_string())?;
    let scope = robit_agent::schedule::TaskScope {
        source: "gui".to_string(),
        namespace: String::new(),
        chat_id: None,
    };
    let mut task = shared
        .store
        .get_scoped(task_id, &scope)
        .map_err(|e| format!("Store error: {}", e))?
        .ok_or_else(|| format!("Task '{}' not found", task_id))?;
    if task.status == robit_agent::schedule::TaskStatus::Deleted {
        return Err(format!("Task '{}' is already deleted", task_id));
    }
    let expected = robit_agent::schedule::fmt_ts(task.updated_at);
    let now = chrono::Utc::now();
    match action {
        "pause" => {
            task.status = robit_agent::schedule::TaskStatus::Paused;
            task.next_run_at = None;
        }
        "resume" => {
            let tz = task.tz().map_err(|e| format!("{}", e))?;
            task.next_run_at = task.schedule.next_after(now, &tz, task.created_at);
            task.status = if task.next_run_at.is_none() {
                robit_agent::schedule::TaskStatus::Completed
            } else {
                robit_agent::schedule::TaskStatus::Active
            };
        }
        "delete" => {
            task.status = robit_agent::schedule::TaskStatus::Deleted;
            task.next_run_at = None;
        }
        _ => return Err(format!("Unknown action: {}", action)),
    }
    task.updated_at = now;
    match shared.store.save_expected(&task, &expected) {
        Ok(true) => {
            shared.wake.notify_one();
            Ok(())
        }
        Ok(false) => Err("Conflict: task changed concurrently, please retry".to_string()),
        Err(e) => Err(format!("Store error: {}", e)),
    }
}
