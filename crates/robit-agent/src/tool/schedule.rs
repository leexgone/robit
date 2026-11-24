//! Scheduled-task tools: `schedule_create` / `schedule_edit` /
//! `schedule_list` / `schedule_view`.
//!
//! Registration is gated by `[app.schedule] enabled` (bootstrap), and the
//! tools are system tools — the `enabled_tools` whitelist does not gate them
//! (same discipline as the memory tools under `memory_mode`). All four read
//! the shared [`ScheduleContext`] from `ToolContext`; the binding carried
//! there scopes every query so one chat can never see or modify another
//! chat's tasks.
//!
//! Confirmation policy (RFC 拍板点 E): create/edit require user confirmation
//! (persistent behavior change); list/view are read-only and free.

use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::Result;
use crate::schedule::{
    fmt_ts, local_display, resolve_tz, ScheduleContext, ScheduleSpec, ScheduledTask, TargetSpec,
    TaskStatus,
};
use crate::tool::{Tool, ToolContext, ToolResult};

/// Max title length (chars) and prompt length (bytes) — quota guards.
const MAX_TITLE_CHARS: usize = 120;
const MAX_PROMPT_BYTES: usize = 8192;

fn sched_ctx<'a>(ctx: &'a ToolContext) -> std::result::Result<&'a ScheduleContext, ToolResult> {
    ctx.schedule.as_ref().ok_or_else(|| {
        ToolResult::error("Scheduled tasks are not available in this session (host did not attach a schedule context).")
    })
}

// ============================================================================
// schedule_create
// ============================================================================

pub struct ScheduleCreateTool;

impl ScheduleCreateTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ScheduleCreateTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct CreateArgs {
    title: String,
    prompt: String,
    schedule: Value,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    max_runs: Option<u32>,
}

#[async_trait]
impl Tool for ScheduleCreateTool {
    fn name(&self) -> &str {
        "schedule_create"
    }

    fn description(&self) -> &str {
        "Create a scheduled task: a prompt delivered to THIS session as a user turn at the \
         configured time(s). ONLY create when the user EXPLICITLY asks for a reminder, a \
         timed action, or a recurring job — never register tasks on your own initiative \
         from things the user merely mentioned. The prompt must be self-contained (at fire \
         time there is no fresh conversation context to rely on). Trigger 'schedule' is one \
         of: {\"kind\":\"once\",\"at\":\"2026-11-25T09:00:00+08:00\"} (RFC3339 with offset, \
         future); {\"kind\":\"interval\",\"every_secs\":3600} (optional RFC3339 'anchor'); \
         {\"kind\":\"cron\",\"expr\":\"0 9 * * 1-5\"} (5 fields: min hour dom mon dow, evaluated \
         in 'timezone'). Requires user confirmation."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short task name in the user's language (<=120 chars); no timing/status/id in it."
                },
                "prompt": {
                    "type": "string",
                    "description": "The full self-contained instruction delivered at each fire."
                },
                "schedule": {
                    "type": "object",
                    "description": "Trigger: {kind:'once',at} | {kind:'interval',every_secs,anchor?} | {kind:'cron',expr}.",
                    "properties": {
                        "kind": { "type": "string", "enum": ["once", "interval", "cron"] },
                        "at": { "type": "string" },
                        "every_secs": { "type": "integer" },
                        "anchor": { "type": "string" },
                        "expr": { "type": "string" }
                    },
                    "required": ["kind"]
                },
                "timezone": {
                    "type": "string",
                    "description": "IANA timezone (e.g. Asia/Shanghai). Default: host local."
                },
                "max_runs": {
                    "type": "integer",
                    "description": "Optional cap on total fires; task completes when reached."
                }
            },
            "required": ["title", "prompt", "schedule"]
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let sc = match sched_ctx(ctx) {
            Ok(sc) => sc,
            Err(r) => return Ok(r),
        };
        let args: CreateArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Invalid arguments: {}", e))),
        };

        // Quota / shape guards.
        if args.title.trim().is_empty() || args.title.chars().count() > MAX_TITLE_CHARS {
            return Ok(ToolResult::error(format!(
                "title must be non-empty and at most {} characters",
                MAX_TITLE_CHARS
            )));
        }
        if args.prompt.trim().is_empty() || args.prompt.len() > MAX_PROMPT_BYTES {
            return Ok(ToolResult::error(format!(
                "prompt must be non-empty and at most {} bytes",
                MAX_PROMPT_BYTES
            )));
        }
        let spec: ScheduleSpec = match serde_json::from_value(args.schedule) {
            Ok(s) => s,
            Err(e) => {
                return Ok(ToolResult::error(format!(
                    "Invalid schedule (need kind=once|interval|cron): {}",
                    e
                )))
            }
        };
        let now = Utc::now();
        if let Err(e) = spec.validate(now, sc.settings.min_interval_secs) {
            return Ok(ToolResult::error(e));
        }
        let tz_name = args.timezone.unwrap_or_default();
        let tz = match resolve_tz(&tz_name) {
            Ok(tz) => tz,
            Err(e) => return Ok(ToolResult::error(e)),
        };
        let count = match sc.store.count_unfinished(&sc.binding.scope()) {
            Ok(n) => n,
            Err(e) => return Ok(ToolResult::error(format!("Store error: {}", e))),
        };
        if count >= sc.settings.max_active_tasks {
            return Ok(ToolResult::error(format!(
                "Task quota reached ({} active/paused tasks); delete or complete some first",
                sc.settings.max_active_tasks
            )));
        }
        let Some(next) = spec.next_after(now, &tz, now) else {
            return Ok(ToolResult::error("Trigger has no upcoming occurrence"));
        };

        let task = ScheduledTask {
            id: uuid::Uuid::new_v4().to_string(),
            title: args.title.trim().to_string(),
            prompt: args.prompt,
            schedule: spec,
            timezone: tz_name,
            target: TargetSpec::ThisSession {
                session_id: sc.binding.session_id.clone(),
                chat_id: sc.binding.chat_id.clone(),
            },
            source: sc.binding.source.clone(),
            namespace: sc.binding.namespace.clone(),
            status: TaskStatus::Active,
            max_runs: args.max_runs,
            run_count: 0,
            next_run_at: Some(next),
            last_run_at: None,
            created_at: now,
            updated_at: now,
        };
        if let Err(e) = sc.store.insert(&task) {
            return Ok(ToolResult::error(format!("Store error: {}", e)));
        }
        sc.wake.notify_one();
        Ok(ToolResult::success(format!(
            "Created scheduled task.\nid: {}\ntitle: {}\nnext fire: {} (UTC {})",
            task.id,
            task.title,
            local_display(next),
            fmt_ts(next)
        )))
    }
}

// ============================================================================
// schedule_edit
// ============================================================================

pub struct ScheduleEditTool;

impl ScheduleEditTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ScheduleEditTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct EditArgs {
    id: String,
    action: String,
    /// Optimistic concurrency token from schedule_view/list (updated_at).
    expected_updated_at: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    schedule: Option<Value>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    max_runs: Option<u32>,
}

#[async_trait]
impl Tool for ScheduleEditTool {
    fn name(&self) -> &str {
        "schedule_edit"
    }

    fn description(&self) -> &str {
        "Update, pause, resume, or delete a scheduled task of THIS session/chat. \
         Pass expected_updated_at from schedule_view/schedule_list; on conflict re-read \
         and retry. action='update' may change title/prompt/schedule/timezone/max_runs. \
         Deleting is soft (audit kept). Requires user confirmation."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "action": { "type": "string", "enum": ["update", "pause", "resume", "delete"] },
                "expected_updated_at": {
                    "type": "string",
                    "description": "The task's updated_at (UTC RFC3339) as last read."
                },
                "title": { "type": "string" },
                "prompt": { "type": "string" },
                "schedule": { "type": "object" },
                "timezone": { "type": "string" },
                "max_runs": { "type": "integer" }
            },
            "required": ["id", "action", "expected_updated_at"]
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let sc = match sched_ctx(ctx) {
            Ok(sc) => sc,
            Err(r) => return Ok(r),
        };
        let args: EditArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Invalid arguments: {}", e))),
        };
        let mut task = match sc.store.get_scoped(&args.id, &sc.binding.scope()) {
            Ok(Some(t)) => t,
            Ok(None) => {
                return Ok(ToolResult::error(format!(
                    "Task '{}' not found in this session/chat",
                    args.id
                )))
            }
            Err(e) => return Ok(ToolResult::error(format!("Store error: {}", e))),
        };
        if task.status == TaskStatus::Deleted {
            return Ok(ToolResult::error(format!("Task '{}' is deleted", args.id)));
        }
        let expected = fmt_ts(task.updated_at);
        if args.expected_updated_at != expected {
            return Ok(ToolResult::error(format!(
                "Conflict: task changed since last read (expected updated_at {}, now {}); \
                 re-read with schedule_view and retry",
                args.expected_updated_at, expected
            )));
        }

        let now = Utc::now();
        match args.action.as_str() {
            "update" => {
                if let Some(t) = args.title {
                    if t.trim().is_empty() || t.chars().count() > MAX_TITLE_CHARS {
                        return Ok(ToolResult::error("Invalid title"));
                    }
                    task.title = t.trim().to_string();
                }
                if let Some(p) = args.prompt {
                    if p.trim().is_empty() || p.len() > MAX_PROMPT_BYTES {
                        return Ok(ToolResult::error("Invalid prompt"));
                    }
                    task.prompt = p;
                }
                if let Some(s) = args.schedule {
                    match serde_json::from_value::<ScheduleSpec>(s) {
                        Ok(spec) => task.schedule = spec,
                        Err(e) => {
                            return Ok(ToolResult::error(format!("Invalid schedule: {}", e)))
                        }
                    }
                }
                if let Some(tz) = args.timezone {
                    if let Err(e) = resolve_tz(&tz) {
                        return Ok(ToolResult::error(e));
                    }
                    task.timezone = tz;
                }
                if let Some(m) = args.max_runs {
                    task.max_runs = Some(m);
                }
                if let Err(e) = task.schedule.validate(now, sc.settings.min_interval_secs) {
                    // A spent `once` is legal when pausing/completed; for an
                    // active update it must still have a future occurrence.
                    if task.status == TaskStatus::Active {
                        return Ok(ToolResult::error(e));
                    }
                }
                if task.status == TaskStatus::Active {
                    let tz = match task.tz() {
                        Ok(tz) => tz,
                        Err(e) => return Ok(ToolResult::error(e.to_string())),
                    };
                    task.next_run_at = task.schedule.next_after(now, &tz, task.created_at);
                    if task.next_run_at.is_none() {
                        task.status = TaskStatus::Completed;
                    }
                }
            }
            "pause" => {
                task.status = TaskStatus::Paused;
                task.next_run_at = None;
            }
            "resume" => {
                let tz = match task.tz() {
                    Ok(tz) => tz,
                    Err(e) => return Ok(ToolResult::error(e.to_string())),
                };
                task.next_run_at = task.schedule.next_after(now, &tz, task.created_at);
                task.status = if task.next_run_at.is_none() {
                    TaskStatus::Completed
                } else {
                    TaskStatus::Active
                };
            }
            "delete" => {
                task.status = TaskStatus::Deleted;
                task.next_run_at = None;
            }
            other => {
                return Ok(ToolResult::error(format!(
                    "Unknown action '{}' (use update|pause|resume|delete)",
                    other
                )))
            }
        }
        task.updated_at = now;
        match sc.store.save_expected(&task, &expected) {
            Ok(true) => {}
            Ok(false) => {
                return Ok(ToolResult::error(
                    "Conflict: task changed concurrently; re-read with schedule_view and retry",
                ))
            }
            Err(e) => return Ok(ToolResult::error(format!("Store error: {}", e))),
        }
        sc.wake.notify_one();
        let next = task
            .next_run_at
            .map(|dt| format!("{} (UTC {})", local_display(dt), fmt_ts(dt)))
            .unwrap_or_else(|| "-".to_string());
        Ok(ToolResult::success(format!(
            "Task {} {}.\nstatus: {}\nnext fire: {}",
            task.id,
            args.action,
            task.status.as_str(),
            next
        )))
    }
}

// ============================================================================
// schedule_list
// ============================================================================

pub struct ScheduleListTool;

impl ScheduleListTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ScheduleListTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct ListArgs {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
}

#[async_trait]
impl Tool for ScheduleListTool {
    fn name(&self) -> &str {
        "schedule_list"
    }

    fn description(&self) -> &str {
        "List scheduled tasks of THIS session/chat (id, title, status, next fire, run count). \
         Read-only, no confirmation. Deleted tasks are hidden."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": { "type": "string", "enum": ["active", "paused", "completed"] },
                "limit": { "type": "integer", "description": "Default 20, max 100." },
                "offset": { "type": "integer" }
            }
        })
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let sc = match sched_ctx(ctx) {
            Ok(sc) => sc,
            Err(r) => return Ok(r),
        };
        let args: ListArgs = serde_json::from_value(args).unwrap_or(ListArgs {
            status: None,
            limit: None,
            offset: None,
        });
        let status = match args.status.as_deref() {
            None => None,
            Some(s) => match TaskStatus::parse(s) {
                Some(st) if st != TaskStatus::Deleted => Some(st),
                _ => return Ok(ToolResult::error(format!("Invalid status filter: {}", s))),
            },
        };
        let limit = args.limit.unwrap_or(20).min(100);
        let tasks = match sc
            .store
            .list_scoped(&sc.binding.scope(), status, limit, args.offset.unwrap_or(0))
        {
            Ok(t) => t,
            Err(e) => return Ok(ToolResult::error(format!("Store error: {}", e))),
        };
        if tasks.is_empty() {
            return Ok(ToolResult::success("No scheduled tasks in this session/chat."));
        }
        let mut out = String::from("Scheduled tasks:\n");
        for t in tasks {
            let next = t
                .next_run_at
                .map(local_display)
                .unwrap_or_else(|| "-".to_string());
            out.push_str(&format!(
                "- {} | {} | {} | next: {} | runs: {} | updated_at: {}\n",
                t.id,
                t.title,
                t.status.as_str(),
                next,
                t.run_count,
                fmt_ts(t.updated_at)
            ));
        }
        Ok(ToolResult::success(out))
    }
}

// ============================================================================
// schedule_view
// ============================================================================

pub struct ScheduleViewTool;

impl ScheduleViewTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ScheduleViewTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct ViewArgs {
    id: String,
}

#[async_trait]
impl Tool for ScheduleViewTool {
    fn name(&self) -> &str {
        "schedule_view"
    }

    fn description(&self) -> &str {
        "Show one scheduled task of THIS session/chat in full (trigger, prompt, target, \
         updated_at token for schedule_edit) plus its 10 most recent run records. \
         Read-only, no confirmation."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "id": { "type": "string" } },
            "required": ["id"]
        })
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let sc = match sched_ctx(ctx) {
            Ok(sc) => sc,
            Err(r) => return Ok(r),
        };
        let args: ViewArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Invalid arguments: {}", e))),
        };
        let task = match sc.store.get_scoped(&args.id, &sc.binding.scope()) {
            Ok(Some(t)) if t.status != TaskStatus::Deleted => t,
            Ok(_) => {
                return Ok(ToolResult::error(format!(
                    "Task '{}' not found in this session/chat",
                    args.id
                )))
            }
            Err(e) => return Ok(ToolResult::error(format!("Store error: {}", e))),
        };
        let runs = sc.store.runs_for(&task.id, 10).unwrap_or_default();
        let mut out = format!(
            "id: {}\ntitle: {}\nstatus: {}\nschedule: {}\ntimezone: {}\nmax_runs: {}\nrun_count: {}\n\
             next fire: {}\nlast fire: {}\ncreated_at: {}\nupdated_at: {}\nprompt:\n{}\n",
            task.id,
            task.title,
            task.status.as_str(),
            serde_json::to_string(&task.schedule).unwrap_or_default(),
            if task.timezone.is_empty() { "(host local)" } else { &task.timezone },
            task.max_runs.map(|m| m.to_string()).unwrap_or_else(|| "-".into()),
            task.run_count,
            task.next_run_at.map(|dt| format!("{} (UTC {})", local_display(dt), fmt_ts(dt)))
                .unwrap_or_else(|| "-".into()),
            task.last_run_at.map(|dt| format!("{} (UTC {})", local_display(dt), fmt_ts(dt)))
                .unwrap_or_else(|| "-".into()),
            fmt_ts(task.created_at),
            fmt_ts(task.updated_at),
            task.prompt,
        );
        if !runs.is_empty() {
            out.push_str("recent runs:\n");
            for r in runs {
                out.push_str(&format!(
                    "- scheduled {} fired {} | {}{}{}\n",
                    fmt_ts(r.scheduled_at),
                    fmt_ts(r.fired_at),
                    r.status,
                    if r.catch_up { " (catch-up)" } else { "" },
                    r.error
                        .as_deref()
                        .map(|e| format!(" | error: {}", e))
                        .unwrap_or_default()
                ));
            }
        }
        Ok(ToolResult::success(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::{MissedPolicy, ScheduleBinding, ScheduleSettings, ScheduleStore};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::Notify;

    struct DummyFrontend;
    #[async_trait]
    impl crate::frontend::Frontend for DummyFrontend {
        async fn on_event(&self, _: crate::event::AgentEvent) -> Result<()> {
            Ok(())
        }
        async fn request_tool_confirmation(&self, _: &crate::tool::ToolCallInfo) -> Result<bool> {
            Ok(true)
        }
    }

    fn make_ctx(chat_id: &str) -> ToolContext {
        let (done_tx, _done_rx) = tokio::sync::mpsc::channel(1);
        let store = ScheduleStore::open_in_memory().unwrap();
        ToolContext {
            working_dir: PathBuf::from("."),
            memory_dir: PathBuf::from("."),
            session_id: "sess".to_string(),
            tool_call_id: "tc".to_string(),
            frontend: Arc::new(DummyFrontend),
            extensions: std::collections::HashMap::new(),
            supports_images: false,
            async_runner: crate::tool::async_runner::AsyncTaskRunner::new(done_tx),
            cancel_token: tokio_util::sync::CancellationToken::new(),
            task_registry: crate::tool::task_registry::TaskRegistry::new(),
            schedule: Some(ScheduleContext {
                store,
                wake: Arc::new(Notify::new()),
                settings: ScheduleSettings {
                    enabled: true,
                    missed_policy: MissedPolicy::Latest,
                    min_interval_secs: 60,
                    max_active_tasks: 2,
                    delivery_retries: 0,
                },
                binding: ScheduleBinding {
                    source: "qq".into(),
                    namespace: String::new(),
                    session_id: "sess".into(),
                    chat_id: Some(chat_id.into()),
                },
            }),
        }
    }

    fn create_args(title: &str) -> Value {
        json!({
            "title": title,
            "prompt": "生成日报并发到群里",
            "schedule": { "kind": "cron", "expr": "0 9 * * 1-5" },
            "timezone": "Asia/Shanghai"
        })
    }

    fn extract_id(result: &ToolResult) -> String {
        result
            .content
            .lines()
            .find(|l| l.starts_with("id: "))
            .unwrap()
            .trim_start_matches("id: ")
            .to_string()
    }

    #[tokio::test]
    async fn create_list_view_edit_flow() {
        let ctx = make_ctx("group:1");
        let created = ScheduleCreateTool::new()
            .execute(create_args("每日日报"), &ctx)
            .await
            .unwrap();
        assert!(!created.is_error, "{}", created.content);
        let id = extract_id(&created);

        let listed = ScheduleListTool::new()
            .execute(json!({}), &ctx)
            .await
            .unwrap();
        assert!(listed.content.contains(&id));
        assert!(listed.content.contains("每日日报"));

        let viewed = ScheduleViewTool::new()
            .execute(json!({ "id": id }), &ctx)
            .await
            .unwrap();
        assert!(viewed.content.contains("0 9 * * 1-5"));
        let updated_at = viewed
            .content
            .lines()
            .find(|l| l.starts_with("updated_at: "))
            .unwrap()
            .trim_start_matches("updated_at: ")
            .to_string();

        // Pause with correct token.
        let paused = ScheduleEditTool::new()
            .execute(
                json!({ "id": id, "action": "pause", "expected_updated_at": updated_at }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!paused.is_error, "{}", paused.content);
        assert!(paused.content.contains("paused"));

        // Stale token → conflict.
        let conflict = ScheduleEditTool::new()
            .execute(
                json!({ "id": id, "action": "resume", "expected_updated_at": updated_at }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(conflict.is_error);
        assert!(conflict.content.contains("Conflict"));

        // Delete hides from list.
        let viewed2 = ScheduleViewTool::new()
            .execute(json!({ "id": id }), &ctx)
            .await
            .unwrap();
        let token2 = viewed2
            .content
            .lines()
            .find(|l| l.starts_with("updated_at: "))
            .unwrap()
            .trim_start_matches("updated_at: ")
            .to_string();
        let deleted = ScheduleEditTool::new()
            .execute(
                json!({ "id": id, "action": "delete", "expected_updated_at": token2 }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!deleted.is_error);
        let listed2 = ScheduleListTool::new()
            .execute(json!({}), &ctx)
            .await
            .unwrap();
        assert!(!listed2.content.contains(&id));
    }

    #[tokio::test]
    async fn create_rejects_bad_trigger_and_quota() {
        let ctx = make_ctx("group:1");
        // Past once.
        let past = ScheduleCreateTool::new()
            .execute(
                json!({
                    "title": "t", "prompt": "p",
                    "schedule": { "kind": "once", "at": "2020-01-01T00:00:00Z" }
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(past.is_error);
        // Sub-minimum interval.
        let fast = ScheduleCreateTool::new()
            .execute(
                json!({
                    "title": "t", "prompt": "p",
                    "schedule": { "kind": "interval", "every_secs": 5 }
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(fast.is_error);
        // Quota (max_active_tasks = 2 in make_ctx).
        for i in 0..2 {
            let ok = ScheduleCreateTool::new()
                .execute(create_args(&format!("t{}", i)), &ctx)
                .await
                .unwrap();
            assert!(!ok.is_error, "{}", ok.content);
        }
        let over = ScheduleCreateTool::new()
            .execute(create_args("over"), &ctx)
            .await
            .unwrap();
        assert!(over.is_error);
        assert!(over.content.contains("quota"));
    }

    #[tokio::test]
    async fn cross_chat_isolation() {
        let ctx1 = make_ctx("group:1");
        let created = ScheduleCreateTool::new()
            .execute(create_args("群1任务"), &ctx1)
            .await
            .unwrap();
        let id = extract_id(&created);
        // Same store, different chat binding → invisible & uneditable.
        let mut ctx2 = make_ctx("group:2");
        ctx2.schedule.as_mut().unwrap().store = ctx1.schedule.as_ref().unwrap().store.clone();
        let viewed = ScheduleViewTool::new()
            .execute(json!({ "id": id }), &ctx2)
            .await
            .unwrap();
        assert!(viewed.is_error);
        let listed = ScheduleListTool::new()
            .execute(json!({}), &ctx2)
            .await
            .unwrap();
        assert!(!listed.content.contains(&id));
    }

    #[tokio::test]
    async fn missing_context_reports_unavailable() {
        let mut ctx = make_ctx("group:1");
        ctx.schedule = None;
        let r = ScheduleCreateTool::new()
            .execute(create_args("t"), &ctx)
            .await
            .unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("not available"));
    }
}
