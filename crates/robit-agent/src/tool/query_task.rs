//! `query_task` tool - inspect the status of async background tasks.
//!
//! Async tools (e.g. `generate_image`) return a pending placeholder and finish
//! in the background. Their final result is reinjected into the conversation
//! as a system notification when they complete, so the LLM normally doesn't
//! need to poll. This tool exists for the cases where the LLM wants to check
//! what is still running or re-read a task's final status (e.g. after several
//! tasks were launched in parallel).
//!
//! The tool is stateless: it reads the per-Agent [`TaskRegistry`] supplied via
//! [`ToolContext`], so a single shared instance works across every session.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::task_registry::{AsyncTaskRecord, AsyncTaskStatus, TaskRegistry};
use super::{Tool, ToolContext, ToolResult};
use crate::error::Result;

#[derive(Debug, Deserialize)]
struct QueryTaskArgs {
    #[serde(default)]
    task_id: Option<String>,
}

pub struct QueryTaskTool;

impl QueryTaskTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for QueryTaskTool {
    fn name(&self) -> &str {
        "query_task"
    }

    fn description(&self) -> &str {
        "Query the status of async background tasks (e.g. image generation). \
         Pass a `task_id` to inspect one task, or omit it to list all currently \
         running (pending) tasks. Final results of completed tasks are already \
         delivered to you as system-notification messages; use this tool only to \
         check what is still in progress or to re-read a task's final status."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Optional task id to inspect. Omit to list all pending tasks."
                }
            }
        })
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let parsed: QueryTaskArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Argument parsing failed: {}", e))),
        };

        let value = match parsed.task_id {
            Some(id) => match ctx.task_registry.get(&id) {
                Some(record) => record_to_json(&record),
                None => json!({
                    "found": false,
                    "task_id": id,
                    "message": "No task with this id is tracked.",
                }),
            },
            None => pending_list_json(ctx.task_registry.clone()),
        };

        let content = serde_json::to_string_pretty(&value)
            .unwrap_or_else(|_| value.to_string());
        Ok(ToolResult::success(content))
    }
}

fn pending_list_json(registry: TaskRegistry) -> Value {
    let pending = registry.list_pending();
    json!({
        "pending_count": pending.len(),
        "pending": pending.iter().map(brief_json).collect::<Vec<_>>(),
    })
}

fn brief_json(r: &AsyncTaskRecord) -> Value {
    json!({
        "task_id": r.task_id,
        "tool": r.tool_name,
        "status": r.status.as_str(),
    })
}

fn record_to_json(r: &AsyncTaskRecord) -> Value {
    json!({
        "found": true,
        "task_id": r.task_id,
        "tool": r.tool_name,
        "status": r.status.as_str(),
        "result": r.result_summary,
    })
}

// Keep AsyncTaskStatus referenced even if serde path changes.
#[allow(dead_code)]
fn _status_used(s: AsyncTaskStatus) -> &'static str {
    s.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::task_registry::AsyncTaskRecord;
    use std::time::Instant;

    fn record(id: &str, status: AsyncTaskStatus, summary: Option<&str>) -> AsyncTaskRecord {
        AsyncTaskRecord {
            task_id: id.into(),
            tool_name: "generate_image".into(),
            tool_call_id: "tc".into(),
            session_id: "sess".into(),
            status,
            started_at: Instant::now(),
            result_summary: summary.map(String::from),
        }
    }

    #[tokio::test]
    async fn list_pending_empty_returns_count_zero() {
        let tool = QueryTaskTool::new();
        let registry = TaskRegistry::new();
        let ctx = make_ctx(registry);
        let res = tool
            .execute(serde_json::json!({}), &ctx)
            .await
            .unwrap();
        assert!(!res.is_error);
        assert!(res.content.contains("\"pending_count\": 0"));
    }

    #[tokio::test]
    async fn list_pending_shows_registered_tasks() {
        let tool = QueryTaskTool::new();
        let registry = TaskRegistry::new();
        registry.register(record("t1", AsyncTaskStatus::Pending, None));
        registry.register(record("t2", AsyncTaskStatus::Pending, None));
        let ctx = make_ctx(registry);
        let res = tool.execute(serde_json::json!({}), &ctx).await.unwrap();
        assert!(res.content.contains("\"pending_count\": 2"));
        assert!(res.content.contains("t1"));
        assert!(res.content.contains("t2"));
    }

    #[tokio::test]
    async fn query_specific_task_returns_summary() {
        let tool = QueryTaskTool::new();
        let registry = TaskRegistry::new();
        registry.register(record(
            "t9",
            AsyncTaskStatus::Completed,
            Some("saved 1 image to images/cat.png"),
        ));
        let ctx = make_ctx(registry);
        let res = tool
            .execute(serde_json::json!({ "task_id": "t9" }), &ctx)
            .await
            .unwrap();
        assert!(res.content.contains("\"found\": true"));
        assert!(res.content.contains("completed"));
        assert!(res.content.contains("cat.png"));
    }

    #[tokio::test]
    async fn query_unknown_task_reports_not_found() {
        let tool = QueryTaskTool::new();
        let ctx = make_ctx(TaskRegistry::new());
        let res = tool
            .execute(serde_json::json!({ "task_id": "nope" }), &ctx)
            .await
            .unwrap();
        assert!(res.content.contains("\"found\": false"));
    }

    // Build a minimal ToolContext for testing. Only task_registry is read by
    // this tool, so the other fields use placeholder values.
    fn make_ctx(registry: TaskRegistry) -> ToolContext {
        use std::path::PathBuf;
        use std::sync::Arc;
        use tokio::sync::mpsc;
        use tokio_util::sync::CancellationToken;

        // A frontend that accepts everything (this tool never calls it).
        struct DummyFrontend;
        #[async_trait]
        impl crate::frontend::Frontend for DummyFrontend {
            async fn on_event(&self, _: crate::event::AgentEvent) -> Result<()> {
                Ok(())
            }
            async fn request_tool_confirmation(
                &self,
                _: &crate::tool::ToolCallInfo,
            ) -> Result<bool> {
                Ok(true)
            }
        }

        let (done_tx, _done_rx) = mpsc::channel(1);
        ToolContext {
            working_dir: PathBuf::from("."),
            session_id: "sess".to_string(),
            tool_call_id: "tc".to_string(),
            frontend: Arc::new(DummyFrontend),
            extensions: std::collections::HashMap::new(),
            supports_images: false,
            async_runner: crate::tool::async_runner::AsyncTaskRunner::new(done_tx),
            cancel_token: CancellationToken::new(),
            task_registry: registry,
        }
    }
}
