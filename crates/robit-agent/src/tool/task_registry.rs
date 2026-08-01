//! Task registry: tracks the lifecycle of async tool tasks for status queries.
//!
//! The `Agent` owns a [`TaskRegistry`] (wrapped in `Arc`, cheap to clone) and
//! injects a clone into every [`ToolContext`](super::ToolContext). The
//! [`query_task`](super::query_task) tool reads it to report task status to the
//! LLM. Because the registry is shared via `ToolContext` (not held by the tool
//! instance), a single stateless `query_task` works across every Agent/session
//! even though `ToolRegistry` itself is shared.
//!
//! Locking: a `std::sync::Mutex` guards the `HashMap`. All operations are pure
//! HashMap reads/writes that never `.await`, so a blocking std mutex is safe
//! and cheaper than a `tokio::sync::Mutex`. Callers must not hold the guard
//! across `.await`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::event::SessionId;

/// Lifecycle state of an async task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncTaskStatus {
    /// Submitted, running in the background.
    Pending,
    /// Finished successfully (final result reinjected into history).
    Completed,
    /// Finished with an error result.
    Failed,
    /// Cancelled before completion.
    Cancelled,
}

impl AsyncTaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AsyncTaskStatus::Pending => "pending",
            AsyncTaskStatus::Completed => "completed",
            AsyncTaskStatus::Failed => "failed",
            AsyncTaskStatus::Cancelled => "cancelled",
        }
    }
}

/// A snapshot of one async task's metadata, used for status queries.
#[derive(Debug, Clone)]
pub struct AsyncTaskRecord {
    pub task_id: String,
    pub tool_name: String,
    pub tool_call_id: String,
    pub session_id: SessionId,
    pub status: AsyncTaskStatus,
    pub started_at: Instant,
    /// Summary of the final result (truncated content), set on completion.
    /// `None` while `Pending`.
    pub result_summary: Option<String>,
}

/// Shared, thread-safe map of task_id -> record.
///
/// Cloning creates a new handle to the same underlying map.
#[derive(Clone, Default)]
pub struct TaskRegistry {
    inner: Arc<Mutex<HashMap<String, AsyncTaskRecord>>>,
}

impl TaskRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a newly submitted task as `Pending`.
    pub fn register(&self, record: AsyncTaskRecord) {
        let mut g = self.inner.lock().expect("task registry lock poisoned");
        g.insert(record.task_id.clone(), record);
    }

    /// Update a task's status (and optional result summary) on completion.
    pub fn update(
        &self,
        task_id: &str,
        status: AsyncTaskStatus,
        result_summary: Option<String>,
    ) {
        let mut g = self.inner.lock().expect("task registry lock poisoned");
        if let Some(r) = g.get_mut(task_id) {
            r.status = status;
            if result_summary.is_some() {
                r.result_summary = result_summary;
            }
        }
    }

    /// Remove a task record (e.g. after it has been queried and aged out).
    pub fn remove(&self, task_id: &str) {
        let mut g = self.inner.lock().expect("task registry lock poisoned");
        g.remove(task_id);
    }

    /// Look up a single task.
    pub fn get(&self, task_id: &str) -> Option<AsyncTaskRecord> {
        self.inner
            .lock()
            .expect("task registry lock poisoned")
            .get(task_id)
            .cloned()
    }

    /// All currently `Pending` tasks.
    pub fn list_pending(&self) -> Vec<AsyncTaskRecord> {
        let g = self.inner.lock().expect("task registry lock poisoned");
        g.values()
            .filter(|r| r.status == AsyncTaskStatus::Pending)
            .cloned()
            .collect()
    }

    /// All known tasks (any status).
    pub fn list_all(&self) -> Vec<AsyncTaskRecord> {
        let g = self.inner.lock().expect("task registry lock poisoned");
        g.values().cloned().collect()
    }

    /// Number of tracked tasks.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("task registry lock poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, status: AsyncTaskStatus) -> AsyncTaskRecord {
        AsyncTaskRecord {
            task_id: id.into(),
            tool_name: "test_tool".into(),
            tool_call_id: "tc".into(),
            session_id: "sess".into(),
            status,
            started_at: Instant::now(),
            result_summary: None,
        }
    }

    #[test]
    fn register_and_get() {
        let reg = TaskRegistry::new();
        reg.register(record("t1", AsyncTaskStatus::Pending));
        assert_eq!(reg.len(), 1);
        let r = reg.get("t1").unwrap();
        assert_eq!(r.status, AsyncTaskStatus::Pending);
    }

    #[test]
    fn update_changes_status_and_summary() {
        let reg = TaskRegistry::new();
        reg.register(record("t1", AsyncTaskStatus::Pending));
        reg.update("t1", AsyncTaskStatus::Completed, Some("ok".into()));
        let r = reg.get("t1").unwrap();
        assert_eq!(r.status, AsyncTaskStatus::Completed);
        assert_eq!(r.result_summary.as_deref(), Some("ok"));
    }

    #[test]
    fn list_pending_filters_by_status() {
        let reg = TaskRegistry::new();
        reg.register(record("t1", AsyncTaskStatus::Pending));
        reg.register(record("t2", AsyncTaskStatus::Completed));
        reg.register(record("t3", AsyncTaskStatus::Pending));
        let pending = reg.list_pending();
        assert_eq!(pending.len(), 2);
        assert!(pending.iter().all(|r| r.status == AsyncTaskStatus::Pending));
    }

    #[test]
    fn remove_drops_record() {
        let reg = TaskRegistry::new();
        reg.register(record("t1", AsyncTaskStatus::Pending));
        reg.remove("t1");
        assert!(reg.get("t1").is_none());
        assert!(reg.is_empty());
    }

    #[test]
    fn clone_shares_state() {
        let reg = TaskRegistry::new();
        let clone = reg.clone();
        reg.register(record("t1", AsyncTaskStatus::Pending));
        // Clone sees the same record.
        assert!(clone.get("t1").is_some());
    }
}
