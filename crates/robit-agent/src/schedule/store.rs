//! Persistence for scheduled tasks (migration v8 tables, see `storage.rs`).
//!
//! Locking discipline mirrors `TaskRegistry`: a `std::sync::Mutex` guards the
//! connection; every operation is short synchronous SQL that never `.await`s
//! while the guard is held. The store opens its **own** connection to the
//! shared `robit.db` (same pattern as the memory tools) with a busy timeout,
//! so host connections and the scheduler never share a mutex.
//!
//! Claim semantics (RFC §5.3/§6.4): advancing `next_run_at` and inserting the
//! `task_runs(status='claimed')` row happen in ONE transaction. A crash
//! between claim and delivery therefore leaves a `claimed` row that startup
//! reconciliation resolves — no silent loss, at most one catch-up fire.

use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{fmt_ts, parse_ts, MissedPolicy, ScheduleSpec, TaskTz, resolve_tz};
use crate::error::{AgentError, Result};
use crate::storage;

/// Host ownership key — mirrors the v7 session identity dimensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    pub source: String,
    pub namespace: String,
}

/// Tool-level query scope: owner plus (for Bot frontends) the current chat.
/// `chat_id = None` means "no chat dimension" (GUI/TUI/Web).
#[derive(Debug, Clone)]
pub struct TaskScope {
    pub source: String,
    pub namespace: String,
    pub chat_id: Option<String>,
}

/// Lifecycle state of a scheduled task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Active,
    Paused,
    Completed,
    /// Soft-deleted: kept for run audit, hidden from lists.
    Deleted,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Deleted => "deleted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "paused" => Some(Self::Paused),
            "completed" => Some(Self::Completed),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }
}

/// Delivery target. `new_session` is reserved for M3 and rejected at creation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TargetSpec {
    ThisSession {
        session_id: String,
        #[serde(default)]
        chat_id: Option<String>,
    },
}

/// A persisted scheduled task.
#[derive(Debug, Clone)]
pub struct ScheduledTask {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub schedule: ScheduleSpec,
    /// IANA timezone name; empty = host local.
    pub timezone: String,
    pub target: TargetSpec,
    pub source: String,
    pub namespace: String,
    pub status: TaskStatus,
    pub max_runs: Option<u32>,
    /// Fires attempted (incremented at claim time, including failed deliveries).
    pub run_count: u32,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ScheduledTask {
    /// Resolve the task timezone (host local when unset).
    pub fn tz(&self) -> Result<TaskTz> {
        resolve_tz(&self.timezone).map_err(AgentError::ConfigError)
    }

    /// Chat id of the delivery target, when bound to a platform chat.
    pub fn target_chat_id(&self) -> Option<&str> {
        match &self.target {
            TargetSpec::ThisSession { chat_id, .. } => chat_id.as_deref(),
        }
    }
}

/// Outcome state of one fire attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// Claimed, delivery result not yet recorded (crash-reconciliation marker).
    Claimed,
    /// Handed to the session channel (NOT "turn executed successfully").
    Delivered,
    Failed,
    Skipped,
}

impl RunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Claimed => "claimed",
            Self::Delivered => "delivered",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

/// One run audit row.
#[derive(Debug, Clone)]
pub struct RunRecord {
    pub id: i64,
    pub task_id: String,
    pub scheduled_at: DateTime<Utc>,
    pub fired_at: DateTime<Utc>,
    pub catch_up: bool,
    pub status: String,
    pub error: Option<String>,
}

/// A claimed fire ready for delivery.
#[derive(Debug, Clone)]
pub struct ClaimedFire {
    /// Task snapshot at claim time.
    pub task: ScheduledTask,
    /// `task_runs` row to finalize via [`ScheduleStore::finish_run`].
    pub run_id: i64,
    /// The occurrence instant this fire stands for (the consumed next_run_at).
    pub scheduled_at: DateTime<Utc>,
    pub catch_up: bool,
}

const TASK_COLS: &str = "id, title, prompt, schedule, timezone, target, source, namespace, \
                         status, max_runs, run_count, next_run_at, last_run_at, created_at, updated_at";

/// Shared store handle. Cloning shares the same connection.
#[derive(Clone)]
pub struct ScheduleStore {
    conn: Arc<Mutex<Connection>>,
}

impl ScheduleStore {
    /// Open (creating parent dirs) and initialize the shared database.
    /// `init_db` is idempotent and brings the schema to the current version,
    /// so the store works even if the host has not opened the DB yet.
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(db_path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        storage::init_db(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// In-memory store for tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        storage::init_db(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("schedule store lock poisoned")
    }

    // ------------------------------------------------------------------
    // CRUD (tool surface, scope-guarded)
    // ------------------------------------------------------------------

    pub fn insert(&self, task: &ScheduledTask) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO scheduled_tasks \
             (id, title, prompt, schedule, timezone, target, source, namespace, status, \
              max_runs, run_count, next_run_at, last_run_at, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                task.id,
                task.title,
                task.prompt,
                serde_json::to_string(&task.schedule)
                    .map_err(|e| AgentError::InternalError(e.to_string()))?,
                task.timezone,
                serde_json::to_string(&task.target)
                    .map_err(|e| AgentError::InternalError(e.to_string()))?,
                task.source,
                task.namespace,
                task.status.as_str(),
                task.max_runs,
                task.run_count,
                task.next_run_at.map(fmt_ts),
                task.last_run_at.map(fmt_ts),
                fmt_ts(task.created_at),
                fmt_ts(task.updated_at),
            ],
        )?;
        Ok(())
    }

    /// Fetch one task within scope; out-of-scope ids read as "not found".
    pub fn get_scoped(&self, id: &str, scope: &TaskScope) -> Result<Option<ScheduledTask>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {TASK_COLS} FROM scheduled_tasks \
             WHERE id = ?1 AND source = ?2 AND namespace = ?3 \
             AND (?4 IS NULL OR json_extract(target, '$.chat_id') = ?4)"
        ))?;
        let row = stmt
            .query_row(
                params![id, scope.source, scope.namespace, scope.chat_id],
                row_to_task,
            )
            .optional()?;
        Ok(row)
    }

    /// List tasks in scope, newest first. `status = None` excludes deleted.
    pub fn list_scoped(
        &self,
        scope: &TaskScope,
        status: Option<TaskStatus>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ScheduledTask>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {TASK_COLS} FROM scheduled_tasks \
             WHERE source = ?1 AND namespace = ?2 \
             AND (?3 IS NULL OR json_extract(target, '$.chat_id') = ?3) \
             AND (?4 IS NULL OR status = ?4) AND status != 'deleted' \
             ORDER BY created_at DESC LIMIT ?5 OFFSET ?6"
        ))?;
        let rows = stmt.query_map(
            params![
                scope.source,
                scope.namespace,
                scope.chat_id,
                status.map(|s| s.as_str()),
                limit as i64,
                offset as i64
            ],
            row_to_task,
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Count active+paused tasks in scope (quota check).
    pub fn count_unfinished(&self, scope: &TaskScope) -> Result<usize> {
        let conn = self.lock();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM scheduled_tasks \
             WHERE source = ?1 AND namespace = ?2 \
             AND (?3 IS NULL OR json_extract(target, '$.chat_id') = ?3) \
             AND status IN ('active', 'paused')",
            params![scope.source, scope.namespace, scope.chat_id],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Persist an edited task with optimistic concurrency: the UPDATE only
    /// lands when `updated_at` still equals `expected`. Returns false on
    /// conflict (the caller re-reads and retries).
    ///
    /// The stored `updated_at` is guaranteed strictly monotonic per row: when
    /// the caller's new timestamp does not exceed the expected one (two edits
    /// inside the same millisecond), it is bumped to expected + 1ms so the
    /// token can never collide.
    pub fn save_expected(&self, task: &ScheduledTask, expected: &str) -> Result<bool> {
        let mut updated = task.updated_at;
        // Compare at STORED precision: two edits inside one millisecond must
        // still bump, even though the raw nanosecond clocks differ.
        if fmt_ts(updated).as_str() <= expected {
            if let Ok(old) = parse_ts(expected) {
                updated = old + chrono::TimeDelta::milliseconds(1);
            }
        }
        let conn = self.lock();
        let changed = conn.execute(
            "UPDATE scheduled_tasks SET title=?1, prompt=?2, schedule=?3, timezone=?4, \
             target=?5, status=?6, max_runs=?7, next_run_at=?8, updated_at=?9 \
             WHERE id=?10 AND updated_at=?11 AND source=?12 AND namespace=?13",
            params![
                task.title,
                task.prompt,
                serde_json::to_string(&task.schedule)
                    .map_err(|e| AgentError::InternalError(e.to_string()))?,
                task.timezone,
                serde_json::to_string(&task.target)
                    .map_err(|e| AgentError::InternalError(e.to_string()))?,
                task.status.as_str(),
                task.max_runs,
                task.next_run_at.map(fmt_ts),
                fmt_ts(updated),
                task.id,
                expected,
                task.source,
                task.namespace,
            ],
        )?;
        Ok(changed == 1)
    }

    // ------------------------------------------------------------------
    // Scheduler surface (owner-guarded)
    // ------------------------------------------------------------------

    /// Claim all due tasks of this owner: advance `next_run_at` and write a
    /// `claimed` run row atomically per task. `catch_up` marks startup
    /// reconciliation fires. `run_count` increments here (fires attempted).
    pub fn claim_due(
        &self,
        owner: &Owner,
        now: DateTime<Utc>,
        catch_up: bool,
    ) -> Result<Vec<ClaimedFire>> {
        let mut conn = self.lock();
        let due: Vec<ScheduledTask> = {
            let mut stmt = conn.prepare(&format!(
                "SELECT {TASK_COLS} FROM scheduled_tasks \
                 WHERE status = 'active' AND next_run_at IS NOT NULL \
                 AND next_run_at <= ?1 AND source = ?2 AND namespace = ?3 \
                 ORDER BY next_run_at ASC"
            ))?;
            let rows = stmt.query_map(params![fmt_ts(now), owner.source, owner.namespace], row_to_task)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let mut fires = Vec::new();
        for task in due {
            let Some(old_next) = task.next_run_at else { continue };
            let tz = match task.tz() {
                Ok(tz) => tz,
                Err(e) => {
                    tracing::error!("schedule task {} has invalid timezone: {}", task.id, e);
                    continue;
                }
            };
            // Advance strictly past `now`: occurrences missed during a long
            // delivery are skipped (latest semantics), never storm-fired.
            let new_next = task.schedule.next_after(now, &tz, task.created_at);
            let new_count = task.run_count + 1;
            let exhausted = new_next.is_none()
                || task.max_runs.map(|m| new_count >= m).unwrap_or(false);
            let new_status = if exhausted { TaskStatus::Completed } else { TaskStatus::Active };
            let new_next = if exhausted { None } else { new_next };

            let tx = conn.transaction()?;
            let changed = tx.execute(
                "UPDATE scheduled_tasks \
                 SET next_run_at=?1, last_run_at=?2, run_count=?3, status=?4, updated_at=?2 \
                 WHERE id=?5 AND status='active' AND next_run_at=?6",
                params![
                    new_next.map(fmt_ts),
                    fmt_ts(now),
                    new_count,
                    new_status.as_str(),
                    task.id,
                    fmt_ts(old_next),
                ],
            )?;
            if changed != 1 {
                // Another claimer won the race — skip without a run row.
                tx.rollback()?;
                continue;
            }
            tx.execute(
                "INSERT INTO task_runs (task_id, scheduled_at, fired_at, catch_up, status) \
                 VALUES (?1, ?2, ?3, ?4, 'claimed')",
                params![task.id, fmt_ts(old_next), fmt_ts(now), catch_up as i64],
            )?;
            let run_id = tx.last_insert_rowid();
            tx.commit()?;

            fires.push(ClaimedFire {
                task,
                run_id,
                scheduled_at: old_next,
                catch_up,
            });
        }
        Ok(fires)
    }

    /// Skip-policy catch-up: advance overdue tasks past `now` without firing,
    /// writing a `skipped` run row for audit.
    pub fn advance_past(&self, owner: &Owner, now: DateTime<Utc>) -> Result<()> {
        let mut conn = self.lock();
        let due: Vec<ScheduledTask> = {
            let mut stmt = conn.prepare(&format!(
                "SELECT {TASK_COLS} FROM scheduled_tasks \
                 WHERE status = 'active' AND next_run_at IS NOT NULL \
                 AND next_run_at <= ?1 AND source = ?2 AND namespace = ?3"
            ))?;
            let rows = stmt.query_map(params![fmt_ts(now), owner.source, owner.namespace], row_to_task)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for task in due {
            let Some(old_next) = task.next_run_at else { continue };
            let tz = match task.tz() {
                Ok(tz) => tz,
                Err(_) => continue,
            };
            let new_next = task.schedule.next_after(now, &tz, task.created_at);
            let exhausted = new_next.is_none()
                || task.max_runs.map(|m| task.run_count >= m).unwrap_or(false);
            let tx = conn.transaction()?;
            let changed = tx.execute(
                "UPDATE scheduled_tasks SET next_run_at=?1, status=?2, updated_at=?3 \
                 WHERE id=?4 AND status='active' AND next_run_at=?5",
                params![
                    if exhausted { None } else { new_next.map(fmt_ts) },
                    if exhausted { TaskStatus::Completed.as_str() } else { TaskStatus::Active.as_str() },
                    fmt_ts(now),
                    task.id,
                    fmt_ts(old_next),
                ],
            )?;
            if changed == 1 {
                tx.execute(
                    "INSERT INTO task_runs (task_id, scheduled_at, fired_at, catch_up, status) \
                     VALUES (?1, ?2, ?3, 0, 'skipped')",
                    params![task.id, fmt_ts(old_next), fmt_ts(now)],
                )?;
            }
            tx.commit()?;
        }
        Ok(())
    }

    /// Resolve `claimed` run rows left behind by a crash between claim and
    /// delivery (RFC §5.3). `Latest` returns them for one catch-up delivery
    /// (the row stays `claimed` until the delivery result finalizes it);
    /// `Skip` marks them skipped. Tasks paused/deleted meanwhile are skipped
    /// under either policy.
    pub fn reconcile_claimed(&self, owner: &Owner, policy: MissedPolicy) -> Result<Vec<ClaimedFire>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT r.id, r.scheduled_at, {TASK_COLS_QUALIFIED} \
             FROM task_runs r JOIN scheduled_tasks t ON t.id = r.task_id \
             WHERE r.status = 'claimed' AND t.source = ?1 AND t.namespace = ?2"
        ))?;
        struct Row {
            run_id: i64,
            scheduled_at: String,
            task: ScheduledTask,
        }
        let rows = stmt.query_map(params![owner.source, owner.namespace], |r| {
            Ok(Row {
                run_id: r.get(0)?,
                scheduled_at: r.get(1)?,
                task: row_to_task_offset(r, 2)?,
            })
        })?;
        let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);

        let mut fires = Vec::new();
        for row in rows {
            let deliverable = matches!(row.task.status, TaskStatus::Active | TaskStatus::Completed);
            if policy == MissedPolicy::Latest && deliverable {
                fires.push(ClaimedFire {
                    scheduled_at: parse_ts(&row.scheduled_at)?,
                    task: row.task,
                    run_id: row.run_id,
                    catch_up: true,
                });
            } else {
                conn.execute(
                    "UPDATE task_runs SET status = 'skipped' WHERE id = ?1",
                    params![row.run_id],
                )?;
            }
        }
        Ok(fires)
    }

    /// Finalize a run row after delivery (or final failure).
    pub fn finish_run(&self, run_id: i64, status: RunStatus, error: Option<&str>) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE task_runs SET status = ?1, error = ?2 WHERE id = ?3",
            params![status.as_str(), error, run_id],
        )?;
        Ok(())
    }

    /// Earliest upcoming fire of this owner (the scheduler's sleep target).
    pub fn min_next_run(&self, owner: &Owner) -> Result<Option<DateTime<Utc>>> {
        let conn = self.lock();
        let s: Option<String> = conn.query_row(
            "SELECT MIN(next_run_at) FROM scheduled_tasks \
             WHERE status = 'active' AND source = ?1 AND namespace = ?2",
            params![owner.source, owner.namespace],
            |r| r.get(0),
        )?;
        s.map(|s| parse_ts(&s)).transpose()
    }

    /// Recent run audit rows of one task, newest first.
    pub fn runs_for(&self, task_id: &str, limit: usize) -> Result<Vec<RunRecord>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, task_id, scheduled_at, fired_at, catch_up, status, error \
             FROM task_runs WHERE task_id = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![task_id, limit as i64], |r| {
            Ok(RunRecord {
                id: r.get(0)?,
                task_id: r.get(1)?,
                scheduled_at: parse_ts(&r.get::<_, String>(2)?)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
                fired_at: parse_ts(&r.get::<_, String>(3)?)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
                catch_up: r.get::<_, i64>(4)? != 0,
                status: r.get(5)?,
                error: r.get(6)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Keep only the newest `keep` run rows of a task.
    pub fn prune_runs(&self, task_id: &str, keep: usize) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "DELETE FROM task_runs WHERE task_id = ?1 AND id NOT IN \
             (SELECT id FROM task_runs WHERE task_id = ?1 ORDER BY id DESC LIMIT ?2)",
            params![task_id, keep as i64],
        )?;
        Ok(())
    }
}

/// Column list qualified for the reconcile join (tasks aliased `t`).
const TASK_COLS_QUALIFIED: &str = "t.id, t.title, t.prompt, t.schedule, t.timezone, t.target, \
    t.source, t.namespace, t.status, t.max_runs, t.run_count, t.next_run_at, t.last_run_at, \
    t.created_at, t.updated_at";

fn row_to_task(r: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledTask> {
    row_to_task_offset(r, 0)
}

fn row_to_task_offset(r: &rusqlite::Row<'_>, o: usize) -> rusqlite::Result<ScheduledTask> {
    let conv = |e: AgentError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
    let schedule_json: String = r.get(o + 3)?;
    let target_json: String = r.get(o + 5)?;
    let status_str: String = r.get(o + 8)?;
    let next: Option<String> = r.get(o + 11)?;
    let last: Option<String> = r.get(o + 12)?;
    Ok(ScheduledTask {
        id: r.get(o)?,
        title: r.get(o + 1)?,
        prompt: r.get(o + 2)?,
        schedule: serde_json::from_str(&schedule_json)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
        timezone: r.get(o + 4)?,
        target: serde_json::from_str(&target_json)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
        source: r.get(o + 6)?,
        namespace: r.get(o + 7)?,
        status: TaskStatus::parse(&status_str)
            .ok_or_else(|| conv(AgentError::InternalError(format!("bad task status: {status_str}"))))?,
        max_runs: r.get(o + 9)?,
        run_count: r.get::<_, i64>(o + 10)? as u32,
        next_run_at: next.map(|s| parse_ts(&s)).transpose().map_err(conv)?,
        last_run_at: last.map(|s| parse_ts(&s)).transpose().map_err(conv)?,
        created_at: parse_ts(&r.get::<_, String>(o + 13)?).map_err(conv)?,
        updated_at: parse_ts(&r.get::<_, String>(o + 14)?).map_err(conv)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::trigger::ScheduleSpec;

    fn owner() -> Owner {
        Owner { source: "qq".into(), namespace: String::new() }
    }

    fn scope() -> TaskScope {
        TaskScope { source: "qq".into(), namespace: String::new(), chat_id: Some("group:1".into()) }
    }

    fn task(id: &str, spec: ScheduleSpec, next: Option<DateTime<Utc>>) -> ScheduledTask {
        let now = Utc::now();
        ScheduledTask {
            id: id.into(),
            title: format!("task {id}"),
            prompt: "do something".into(),
            schedule: spec,
            timezone: "Asia/Shanghai".into(),
            target: TargetSpec::ThisSession {
                session_id: "s-1".into(),
                chat_id: Some("group:1".into()),
            },
            source: "qq".into(),
            namespace: String::new(),
            status: TaskStatus::Active,
            max_runs: None,
            run_count: 0,
            next_run_at: next,
            last_run_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn interval(secs: u64) -> ScheduleSpec {
        ScheduleSpec::Interval { every_secs: secs, anchor: None }
    }

    #[test]
    fn insert_get_and_scope_isolation() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let t = task("t1", interval(3600), Some(Utc::now()));
        store.insert(&t).unwrap();
        assert!(store.get_scoped("t1", &scope()).unwrap().is_some());
        // Other chat / other platform / other instance → invisible.
        let other_chat = TaskScope { chat_id: Some("group:2".into()), ..scope() };
        assert!(store.get_scoped("t1", &other_chat).unwrap().is_none());
        let other_source = TaskScope { source: "wechat".into(), ..scope() };
        assert!(store.get_scoped("t1", &other_source).unwrap().is_none());
        let other_ns = TaskScope { namespace: "inst2".into(), ..scope() };
        assert!(store.get_scoped("t1", &other_ns).unwrap().is_none());
    }

    #[test]
    fn save_expected_optimistic_concurrency() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let t = task("t1", interval(3600), Some(Utc::now()));
        store.insert(&t).unwrap();
        let mut edited = store.get_scoped("t1", &scope()).unwrap().unwrap();
        let expected = fmt_ts(edited.updated_at);
        edited.title = "renamed".into();
        edited.updated_at = Utc::now();
        assert!(store.save_expected(&edited, &expected).unwrap());
        // Stale expectation → conflict, nothing written.
        edited.title = "stale".into();
        assert!(!store.save_expected(&edited, &expected).unwrap());
        assert_eq!(store.get_scoped("t1", &scope()).unwrap().unwrap().title, "renamed");
    }

    #[test]
    fn claim_due_advances_and_writes_claimed_run() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let now = Utc::now();
        let due_at = now - chrono::TimeDelta::seconds(10);
        let t = task("t1", interval(3600), Some(due_at));
        store.insert(&t).unwrap();

        let fires = store.claim_due(&owner(), now, false).unwrap();
        assert_eq!(fires.len(), 1);
        assert_eq!(fmt_ts(fires[0].scheduled_at), fmt_ts(due_at));
        assert!(!fires[0].catch_up);

        let after = store.get_scoped("t1", &scope()).unwrap().unwrap();
        assert_eq!(after.run_count, 1);
        assert!(after.next_run_at.unwrap() > now);
        assert_eq!(after.status, TaskStatus::Active);

        let runs = store.runs_for("t1", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "claimed");

        // Not due again immediately.
        assert!(store.claim_due(&owner(), now, false).unwrap().is_empty());

        store.finish_run(fires[0].run_id, RunStatus::Delivered, None).unwrap();
        assert_eq!(store.runs_for("t1", 10).unwrap()[0].status, "delivered");
    }

    #[test]
    fn claim_respects_owner() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let now = Utc::now();
        store.insert(&task("t1", interval(3600), Some(now - chrono::TimeDelta::seconds(1)))).unwrap();
        let other = Owner { source: "gui".into(), namespace: String::new() };
        assert!(store.claim_due(&other, now, false).unwrap().is_empty());
    }

    #[test]
    fn once_completes_after_fire() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let now = Utc::now();
        let at = now + chrono::TimeDelta::seconds(1);
        let spec = ScheduleSpec::Once { at: fmt_ts(at) };
        store.insert(&task("t1", spec, Some(at))).unwrap();
        let fires = store.claim_due(&owner(), at + chrono::TimeDelta::seconds(1), false).unwrap();
        assert_eq!(fires.len(), 1);
        let after = store.get_scoped("t1", &scope()).unwrap().unwrap();
        assert_eq!(after.status, TaskStatus::Completed);
        assert!(after.next_run_at.is_none());
    }

    #[test]
    fn max_runs_completes() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let now = Utc::now();
        let mut t = task("t1", interval(60), Some(now - chrono::TimeDelta::seconds(1)));
        t.max_runs = Some(1);
        store.insert(&t).unwrap();
        store.claim_due(&owner(), now, false).unwrap();
        let after = store.get_scoped("t1", &scope()).unwrap().unwrap();
        assert_eq!(after.status, TaskStatus::Completed);
    }

    #[test]
    fn reconcile_claimed_latest_refires_skip_marks() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let now = Utc::now();
        store.insert(&task("t1", interval(3600), Some(now - chrono::TimeDelta::seconds(1)))).unwrap();
        let fires = store.claim_due(&owner(), now, false).unwrap();
        assert_eq!(fires.len(), 1);
        // Simulate crash: run row stays 'claimed'.

        // Skip policy → marked skipped, nothing to deliver.
        let store2 = ScheduleStore::open_in_memory().unwrap();
        store2.insert(&task("t2", interval(3600), Some(now - chrono::TimeDelta::seconds(1)))).unwrap();
        store2.claim_due(&owner(), now, false).unwrap();
        assert!(store2.reconcile_claimed(&owner(), MissedPolicy::Skip).unwrap().is_empty());
        assert_eq!(store2.runs_for("t2", 10).unwrap()[0].status, "skipped");

        // Latest policy → returned for catch-up delivery, catch_up marked.
        let back = store.reconcile_claimed(&owner(), MissedPolicy::Latest).unwrap();
        assert_eq!(back.len(), 1);
        assert!(back[0].catch_up);
        assert_eq!(back[0].run_id, fires[0].run_id);
    }

    #[test]
    fn reconcile_skips_paused_tasks() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let now = Utc::now();
        store.insert(&task("t1", interval(3600), Some(now - chrono::TimeDelta::seconds(1)))).unwrap();
        store.claim_due(&owner(), now, false).unwrap();
        let mut t = store.get_scoped("t1", &scope()).unwrap().unwrap();
        let expected = fmt_ts(t.updated_at);
        t.status = TaskStatus::Paused;
        t.updated_at = Utc::now();
        store.save_expected(&t, &expected).unwrap();
        assert!(store.reconcile_claimed(&owner(), MissedPolicy::Latest).unwrap().is_empty());
        assert_eq!(store.runs_for("t1", 10).unwrap()[0].status, "skipped");
    }

    #[test]
    fn advance_past_skips_overdue_without_firing() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let now = Utc::now();
        store.insert(&task("t1", interval(3600), Some(now - chrono::TimeDelta::seconds(7200)))).unwrap();
        store.advance_past(&owner(), now).unwrap();
        let after = store.get_scoped("t1", &scope()).unwrap().unwrap();
        assert!(after.next_run_at.unwrap() > now);
        assert_eq!(after.run_count, 0); // no fire attempted
        assert_eq!(store.runs_for("t1", 10).unwrap()[0].status, "skipped");
    }

    #[test]
    fn count_and_list_hide_deleted() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let now = Utc::now();
        store.insert(&task("t1", interval(3600), Some(now))).unwrap();
        let mut t2 = task("t2", interval(3600), Some(now));
        t2.status = TaskStatus::Deleted;
        store.insert(&t2).unwrap();
        assert_eq!(store.count_unfinished(&scope()).unwrap(), 1);
        let listed = store.list_scoped(&scope(), None, 20, 0).unwrap();
        assert_eq!(listed.len(), 1); // deleted hidden
        assert_eq!(store.list_scoped(&scope(), Some(TaskStatus::Active), 20, 0).unwrap().len(), 1);
    }

    #[test]
    fn prune_runs_keeps_newest() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let now = Utc::now();
        store.insert(&task("t1", interval(60), Some(now))).unwrap();
        for k in 0..10 {
            let at = now + chrono::TimeDelta::seconds(60 * k + 1);
            let fires = store.claim_due(&owner(), at, false).unwrap();
            assert_eq!(fires.len(), 1, "fire {} should be due", k);
            for f in fires {
                store.finish_run(f.run_id, RunStatus::Delivered, None).unwrap();
            }
        }
        assert_eq!(store.runs_for("t1", 100).unwrap().len(), 10);
        store.prune_runs("t1", 3).unwrap();
        assert_eq!(store.runs_for("t1", 100).unwrap().len(), 3);
    }

    #[test]
    fn min_next_run_owner_scoped() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let now = Utc::now();
        assert!(store.min_next_run(&owner()).unwrap().is_none());
        store.insert(&task("t1", interval(3600), Some(now + chrono::TimeDelta::seconds(60)))).unwrap();
        assert!(store.min_next_run(&owner()).unwrap().is_some());
        let other = Owner { source: "gui".into(), namespace: String::new() };
        assert!(store.min_next_run(&other).unwrap().is_none());
    }
}
