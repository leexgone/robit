//! Scheduled tasks (计划任务) — persistent triggers + delivery abstraction.
//!
//! Design: `docs/robit-rfc-scheduled-tasks.md`. A scheduled task is a persisted
//! prompt that fires as a user turn into its owning session at the configured
//! time(s). The module owns **storage + triggering + delivery abstraction**
//! only; the [`Scheduler`] loop is owned by the host process (Bot manager,
//! GUI/TUI app), never by an `Agent` (agents are per-session and get reclaimed
//! when idle). Agents interact through the four `schedule_*` tools, which read
//! a [`ScheduleContext`] from `ToolContext` (same shared-handle pattern as
//! `TaskRegistry`).
//!
//! Ownership mirrors the v7 session identity key: every task row carries
//! `(source, namespace)` and each host's scheduler only claims its own rows,
//! so frontends sharing one database never fire each other's tasks.
//!
//! Time discipline: all persisted timestamps are UTC RFC3339 strings
//! (`…Z`, second precision — lexicographically sortable). Cron expressions
//! evaluate in the task's IANA timezone (or host local when unset) and the
//! result is converted back to UTC for storage.

pub mod scheduler;
pub mod store;
pub mod trigger;

use std::sync::Arc;

use chrono::{DateTime, Utc};
use robit_ai::config::RobitConfig;
use tokio::sync::Notify;

pub use scheduler::{FireInfo, ScheduleDelivery, Scheduler};
pub use store::{ClaimedFire, Owner, RunRecord, RunStatus, ScheduledTask, ScheduleStore, TargetSpec, TaskScope, TaskStatus};
pub use trigger::{resolve_tz, ScheduleSpec, TaskTz};

use crate::error::{AgentError, Result};

// ============================================================================
// Settings
// ============================================================================

/// Downtime catch-up policy for occurrences missed while the process was down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissedPolicy {
    /// Fire only the most recent missed occurrence (`catch_up = 1`), then
    /// advance to the next future occurrence. Default.
    Latest,
    /// Skip all missed occurrences; advance to the next future occurrence.
    Skip,
}

/// Resolved `[app.schedule]` settings (defaults applied).
#[derive(Debug, Clone)]
pub struct ScheduleSettings {
    pub enabled: bool,
    pub missed_policy: MissedPolicy,
    pub min_interval_secs: u64,
    pub max_active_tasks: usize,
    pub delivery_retries: u32,
}

impl Default for ScheduleSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            missed_policy: MissedPolicy::Latest,
            min_interval_secs: 60,
            max_active_tasks: 50,
            delivery_retries: 3,
        }
    }
}

impl ScheduleSettings {
    /// Resolve from config; absent section = fully disabled defaults.
    pub fn from_config(config: &RobitConfig) -> Self {
        let mut s = Self::default();
        let Some(sec) = config.app.as_ref().and_then(|a| a.schedule.as_ref()) else {
            return s;
        };
        s.enabled = sec.enabled.unwrap_or(false);
        s.missed_policy = match sec.missed_policy.as_deref() {
            Some("skip") => MissedPolicy::Skip,
            Some("latest") | None => MissedPolicy::Latest,
            Some(other) => {
                tracing::warn!(
                    "Unknown [app.schedule] missed_policy '{}', falling back to 'latest'",
                    other
                );
                MissedPolicy::Latest
            }
        };
        if let Some(v) = sec.min_interval_secs {
            s.min_interval_secs = v.max(1);
        }
        if let Some(v) = sec.max_active_tasks {
            s.max_active_tasks = v;
        }
        if let Some(v) = sec.delivery_retries {
            s.delivery_retries = v;
        }
        s
    }
}

// ============================================================================
// Binding / context (shared-handle pattern, cf. TaskRegistry)
// ============================================================================

/// The session/chat a tool invocation belongs to. Fills task ownership and
/// the `this_session` delivery target at creation time, and scopes every
/// CRUD query (a chat can never see or modify another chat's tasks).
#[derive(Debug, Clone)]
pub struct ScheduleBinding {
    /// Frontend platform identifier ("qq", "wechat", "gui", "tui", "web").
    pub source: String,
    /// Instance namespace (`[app.bot] instance_id`, default "").
    pub namespace: String,
    /// Owning session id.
    pub session_id: String,
    /// Platform chat id for Bot frontends; `None` for GUI/TUI/Web.
    pub chat_id: Option<String>,
}

impl ScheduleBinding {
    /// Owner key for scheduler-level queries (all chats of this instance).
    pub fn owner(&self) -> Owner {
        Owner {
            source: self.source.clone(),
            namespace: self.namespace.clone(),
        }
    }

    /// Scope for tool-level queries (this chat/session only).
    pub fn scope(&self) -> TaskScope {
        TaskScope {
            source: self.source.clone(),
            namespace: self.namespace.clone(),
            chat_id: self.chat_id.clone(),
        }
    }
}

/// Handle injected into `ToolContext` when `[app.schedule] enabled = true`.
/// Cheap to clone: the store is an `Arc` around a mutex-guarded connection.
#[derive(Clone)]
pub struct ScheduleContext {
    pub store: ScheduleStore,
    /// Wakes the host's scheduler loop after CRUD so new/changed tasks take
    /// effect without waiting for the old sleep target.
    pub wake: Arc<Notify>,
    pub settings: ScheduleSettings,
    pub binding: ScheduleBinding,
}

// ============================================================================
// Prompt wrapping (shared by all frontends' delivery implementations)
// ============================================================================

/// Wrap a fired task's prompt with the scheduler-origin header. The wrapped
/// text is injected verbatim as a user turn (no slash/skill parsing) and
/// archived with `source = "scheduler"`.
pub fn wrap_prompt(task: &ScheduledTask, fire: &FireInfo) -> String {
    let local = fire
        .fired_at
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M:%S %:z");
    let catch_up = if fire.catch_up { "（停机补触发）" } else { "" };
    format!(
        "[计划任务「{}」(id={}) 于 {} 触发{}]\n{}",
        task.title, task.id, local, catch_up, task.prompt
    )
}

/// Format a UTC instant in host-local time for user-facing display.
pub fn local_display(dt: DateTime<Utc>) -> String {
    dt.with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M:%S %:z")
        .to_string()
}

/// Format a UTC timestamp for storage (millisecond precision, Z suffix).
/// Fixed precision keeps the strings lexicographically sortable; millisecond
/// granularity makes `updated_at` a usable optimistic-concurrency token for
/// back-to-back edits (second precision collided within one second).
pub fn fmt_ts(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Parse a stored UTC timestamp.
pub(crate) fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| AgentError::InternalError(format!("Invalid stored timestamp '{}': {}", s, e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_disabled() {
        let config: RobitConfig = toml::from_str("[providers]\n").unwrap();
        let s = ScheduleSettings::from_config(&config);
        assert!(!s.enabled);
        assert_eq!(s.missed_policy, MissedPolicy::Latest);
        assert_eq!(s.min_interval_secs, 60);
    }

    #[test]
    fn settings_parse_section() {
        let config: RobitConfig = toml::from_str(
            r#"
            [providers]
            [app.schedule]
            enabled = true
            missed_policy = "skip"
            min_interval_secs = 30
            max_active_tasks = 10
            delivery_retries = 1
            "#,
        )
        .unwrap();
        let s = ScheduleSettings::from_config(&config);
        assert!(s.enabled);
        assert_eq!(s.missed_policy, MissedPolicy::Skip);
        assert_eq!(s.min_interval_secs, 30);
        assert_eq!(s.max_active_tasks, 10);
        assert_eq!(s.delivery_retries, 1);
    }

    #[test]
    fn settings_unknown_policy_falls_back() {
        let config: RobitConfig = toml::from_str(
            r#"
            [providers]
            [app.schedule]
            enabled = true
            missed_policy = "bogus"
            "#,
        )
        .unwrap();
        assert_eq!(
            ScheduleSettings::from_config(&config).missed_policy,
            MissedPolicy::Latest
        );
    }

    #[test]
    fn ts_roundtrip() {
        let now = Utc::now();
        let s = fmt_ts(now);
        assert!(s.ends_with('Z'));
        let back = parse_ts(&s).unwrap();
        assert_eq!(fmt_ts(back), s);
    }
}
