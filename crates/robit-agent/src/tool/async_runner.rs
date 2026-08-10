//! Async tool execution: lets long-running tools return a pending placeholder
//! and run their actual work in a background task.
//!
//! A tool that wants to run asynchronously (decided at runtime inside its
//! `execute`) calls [`AsyncTaskRunner::submit`] with the real work as a future
//! and returns a `ToolResult::pending(..)` placeholder. The runner spawns the
//! work as an independent tokio task; when it finishes (or is cancelled) the
//! result is sent back to the Agent via the `done` channel, which reinjects it
//! into the conversation history and wakes the LLM.
//!
//! Cancellation is cooperative: each task owns a `CancellationToken`. When the
//! Agent cancels a task (user `/cancel`, session expiry, or Agent shutdown),
//! the spawned `select!` favors the cancellation branch and the work future is
//! dropped (HTTP requests etc. are cancellation-safe).

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;

use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::event::SessionId;
use crate::tool::ToolResult;

/// A future produced by an async tool representing its background work.
///
/// Resolves to the final `ToolResult` that will be reinjected into the
/// conversation when the task completes.
pub type AsyncTaskWork = BoxFuture<'static, ToolResult>;

/// Message delivered to the Agent when a background task finishes (success,
/// failure, or cancellation).
pub struct AsyncTaskDone {
    pub task_id: String,
    pub tool_call_id: String,
    pub session_id: SessionId,
    pub tool_name: String,
    pub result: ToolResult,
    /// `true` if the task was cancelled (the `cancel` token fired) rather than
    /// completing on its own. Lets the Agent record `Cancelled` rather than
    /// `Failed` status.
    pub cancelled: bool,
}

/// Handle used by tools to submit background work.
///
/// Cheap to clone (just an `mpsc::Sender`); one is placed in every
/// `ToolContext`. The matching `Receiver` lives on the `Agent`, which `select!`s
/// on it alongside user input.
#[derive(Clone)]
pub struct AsyncTaskRunner {
    done_tx: mpsc::Sender<AsyncTaskDone>,
}

impl AsyncTaskRunner {
    /// Create a runner wired to the Agent's `done` channel.
    pub fn new(done_tx: mpsc::Sender<AsyncTaskDone>) -> Self {
        Self { done_tx }
    }

    /// Spawn `work` as a background task and return its `task_id` immediately.
    ///
    /// The caller (the tool's `execute`) should return a `ToolResult::pending`
    /// placeholder using this `task_id`. When `work` resolves - or `cancel` is
    /// triggered - the result is sent to the Agent for reinjection.
    ///
    /// If the Agent has exited, `done_tx.send` fails silently and the result is
    /// dropped (side effects like saved files already happened).
    pub fn submit(
        &self,
        tool_call_id: String,
        session_id: SessionId,
        tool_name: String,
        work: AsyncTaskWork,
        cancel: CancellationToken,
    ) -> String {
        let task_id = uuid::Uuid::new_v4().to_string();
        let done_tx = self.done_tx.clone();
        let tid = task_id.clone();
        let cancelled_msg = format!("异步任务 {} 已取消", tid);

        tokio::spawn(async move {
            // Favor cancellation: if cancelled while still running, return a
            // cancelled result instead of the work's outcome. Dropping `work`
            // cancels any in-flight HTTP requests it holds.
            //
            // `work` is wrapped in `AssertUnwindSafe(...).catch_unwind()` so a
            // panic inside the tool's background future is converted into an
            // error result and delivered to the Agent, instead of aborting the
            // task silently. Without this, a panicking task would never reach
            // `done_tx.send`, leaving a permanently-pending task (query_task
            // stuck, no notification). (Requires panic=unwind, the default;
            // panic=abort would still terminate the process.)
            let (result, cancelled) = tokio::select! {
                biased;
                _ = cancel.cancelled() => (ToolResult::error(cancelled_msg), true),
                r = AssertUnwindSafe(work).catch_unwind() => match r {
                    Ok(tool_result) => (tool_result, false),
                    Err(panic_payload) => {
                        let msg = panic_payload_to_string(&panic_payload);
                        tracing::error!(
                            "[async] background task panicked: task_id={}, tool={}, session={}, panic={}",
                            tid, tool_name, session_id, msg
                        );
                        (
                            ToolResult::error(format!("异步任务执行时发生 panic: {}", msg)),
                            false,
                        )
                    }
                },
            };

            let done = AsyncTaskDone {
                task_id: tid,
                tool_call_id,
                session_id,
                tool_name,
                result,
                cancelled,
            };
            // Agent gone -> drop result. `send` is async but the channel is
            // bounded; use a blocking-style await (spawned task can wait).
            if let Err(send_err) = done_tx.send(done).await {
                // Extract fields from the returned value for logging.
                // Must convert error to string before destructuring (partial move).
                let err_msg = send_err.to_string();
                let lost = send_err.0;
                tracing::error!(
                    "[async] FAILED to deliver task result to Agent (Agent likely exited): \
                     task_id={}, tool={}, session={}, cancelled={}, error={}. \
                     The task result is permanently lost.",
                    lost.task_id, lost.tool_name, lost.session_id, lost.cancelled, err_msg
                );
            }
        });

        task_id
    }
}

/// Best-effort message extraction from a panic payload.
///
/// `panic!("literal")` yields `&'static str`; `panic!("{}", x)` yields `String`.
/// Anything else falls back to a generic placeholder so the log line is still
/// useful.
fn panic_payload_to_string(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

// Keep the raw `Pin<Box<dyn Future>>` alias referenced for documentation;
// `BoxFuture` is the same type with a Send bound.
#[allow(dead_code)]
type _AnyFuture = Pin<Box<dyn Future<Output = ToolResult> + Send>>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;
    use tokio::sync::Mutex;

    async fn recv_n(rx: &Mutex<mpsc::Receiver<AsyncTaskDone>>, n: usize) -> Vec<AsyncTaskDone> {
        let mut out = Vec::new();
        for _ in 0..n {
            match rx.lock().await.recv().await {
                Some(d) => out.push(d),
                None => break,
            }
        }
        out
    }

    #[tokio::test]
    async fn submit_returns_completed_result() {
        let (tx, rx) = mpsc::channel(8);
        let runner = AsyncTaskRunner::new(tx);
        let counter: Arc<StdMutex<u32>> = Arc::new(StdMutex::new(0));
        let c = counter.clone();
        let task_id = runner.submit(
            "tc1".into(),
            "sess".into(),
            "test_tool".into(),
            Box::pin(async move {
                *c.lock().unwrap() += 1;
                ToolResult::success("done")
            }),
            CancellationToken::new(),
        );
        assert!(!task_id.is_empty());

        let results = recv_n(&Mutex::new(rx), 1).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].task_id, task_id);
        assert!(!results[0].result.is_error);
        assert_eq!(results[0].result.content, "done");
        assert_eq!(*counter.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn cancel_returns_cancelled_result() {
        let (tx, rx) = mpsc::channel(8);
        let runner = AsyncTaskRunner::new(tx);
        let cancel = CancellationToken::new();
        let started = Arc::new(tokio::sync::Notify::new());
        let started2 = started.clone();
        let task_id = runner.submit(
            "tc2".into(),
            "sess".into(),
            "test_tool".into(),
            Box::pin(async move {
                started2.notify_one();
                // Simulate long work that should be preempted by cancellation.
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                ToolResult::success("should not reach")
            }),
            cancel.clone(),
        );

        started.notified().await;
        cancel.cancel();

        let results = recv_n(&Mutex::new(rx), 1).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].task_id, task_id);
        assert!(results[0].result.is_error);
        assert!(results[0].result.content.contains("已取消"));
        assert!(results[0].cancelled);
    }

    #[tokio::test]
    async fn panicking_work_delivers_error_result() {
        // A panicking work future must NOT leave the task pending forever.
        // The panic should be caught, logged, and delivered as an error result
        // (cancelled=false) so the Agent can report failure to the LLM.
        let (tx, rx) = mpsc::channel(8);
        let runner = AsyncTaskRunner::new(tx);
        let task_id = runner.submit(
            "tc3".into(),
            "sess".into(),
            "test_tool".into(),
            Box::pin(async {
                panic!("boom from inside work");
            }),
            CancellationToken::new(),
        );

        let results = recv_n(&Mutex::new(rx), 1).await;
        assert_eq!(results.len(), 1, "panic should still deliver a result");
        assert_eq!(results[0].task_id, task_id);
        assert!(results[0].result.is_error);
        assert!(
            results[0].result.content.contains("panic"),
            "content was: {}",
            results[0].result.content
        );
        assert!(
            results[0].result.content.contains("boom from inside work"),
            "content was: {}",
            results[0].result.content
        );
        assert!(!results[0].cancelled);
    }

    #[test]
    fn panic_payload_to_string_handles_common_payloads() {
        let s: Box<dyn std::any::Any + Send> = Box::new("static literal");
        assert_eq!(panic_payload_to_string(&s), "static literal");

        let s: Box<dyn std::any::Any + Send> = Box::new("owned".to_string());
        assert_eq!(panic_payload_to_string(&s), "owned");

        let s: Box<dyn std::any::Any + Send> = Box::new(42i32);
        assert_eq!(panic_payload_to_string(&s), "<non-string panic payload>");
    }
}
