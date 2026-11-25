//! Host-level scheduler wiring for the GUI (计划任务 RFC M2).
//!
//! The scheduler is owned by the Tauri app (one per process), never by an
//! Agent. Delivery routes each fire to the owning session's input channel via
//! the `AppState.agents` registry; a session whose Agent is not live is
//! respawned from the database (same revival semantics as `switch_session`).
//! The wrapped prompt is archived with `source = "scheduler"` before injection.

use std::sync::Arc;

use async_trait::async_trait;
use robit_agent::event::FrontendMessage;
use robit_agent::schedule::{
    wrap_prompt, FireInfo, Owner, ScheduleDelivery, ScheduledTask, Scheduler, TargetSpec,
};
use robit_agent::AgentError;
use tokio_util::sync::CancellationToken;

use crate::db;
use crate::state::AppState;

/// Spawn the GUI scheduler when `[app.schedule] enabled = true`.
/// Returns `None` when the feature is off. The loop ends when `shutdown`
/// is cancelled (Tauri `RunEvent::Exit`).
///
/// Uses `tauri::async_runtime::spawn` because Tauri's `setup` callback runs
/// outside a raw `tokio::runtime::Handle::current()` context — `tokio::spawn`
/// would panic with "no reactor running".
pub fn spawn_scheduler(
    state: Arc<AppState>,
    app_handle: tauri::AppHandle,
    shutdown: CancellationToken,
) -> Option<()> {
    let shared = state.schedule.as_ref()?.clone();
    let delivery = Arc::new(GuiDelivery {
        state: Arc::clone(&state),
        app_handle,
    });
    tracing::info!("GUI scheduler starting (source=gui)");
    tauri::async_runtime::spawn(async move {
        Scheduler::run(
            shared.store.clone(),
            Owner {
                source: "gui".to_string(),
                namespace: String::new(),
            },
            shared.settings.clone(),
            Arc::clone(&shared.wake),
            delivery,
            shutdown,
        )
        .await;
    });
    Some(())
}

/// Delivery terminal: archive + route to the owning session, reviving a
/// closed session from the DB once before giving up.
struct GuiDelivery {
    state: Arc<AppState>,
    app_handle: tauri::AppHandle,
}

#[async_trait]
impl ScheduleDelivery for GuiDelivery {
    async fn deliver(&self, task: &ScheduledTask, fire: &FireInfo) -> robit_agent::error::Result<()> {
        let session_id = match &task.target {
            TargetSpec::ThisSession { session_id, .. } => session_id.clone(),
        };
        let text = wrap_prompt(task, fire);

        // The session must still exist in the DB (deleted session → failed run).
        // Archive the scheduler-origin user row BEFORE injection, mirroring
        // commands::send_message's archive-then-send discipline.
        {
            let db = self.state.db.lock().await;
            let exists = db::get_session(&db, &session_id)
                .map_err(|e| AgentError::InternalError(format!("DB error: {}", e)))?
                .is_some();
            if !exists {
                return Err(AgentError::InternalError(format!(
                    "schedule target session {} no longer exists",
                    session_id
                )));
            }
            db::insert_message(&db, &session_id, "user", &text, None, None, None, Some("scheduler"))
                .map_err(|e| AgentError::InternalError(format!("DB error: {}", e)))?;
            db::touch_session(&db, &session_id)
                .map_err(|e| AgentError::InternalError(format!("DB error: {}", e)))?;
        }

        let make_msg = || FrontendMessage::ScheduledInput {
            task_id: task.id.clone(),
            title: task.title.clone(),
            text: text.clone(),
        };

        // Fast path: live Agent in the registry.
        {
            let agents = self.state.agents.lock().await;
            if let Some(handle) = agents.get(&session_id) {
                if handle.message_tx.send(make_msg()).await.is_ok() {
                    return Ok(());
                }
            }
        }

        // Slow path: respawn from the DB (session closed / dead channel),
        // register the fresh handle, and deliver once more.
        tracing::info!(
            "schedule delivery: respawning session {} for task {}",
            session_id, task.id
        );
        let handle = self
            .state
            .spawn_agent(&session_id, &self.app_handle)
            .await
            .map_err(|e| AgentError::InternalError(format!("respawn failed: {}", e)))?;
        let mut agents = self.state.agents.lock().await;
        agents.insert(session_id.clone(), handle);
        let handle = agents.get(&session_id).expect("handle just inserted");
        handle
            .message_tx
            .send(make_msg())
            .await
            .map_err(|e| AgentError::InternalError(format!("delivery failed after respawn: {}", e)))
    }
}
