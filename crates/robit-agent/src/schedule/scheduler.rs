//! The scheduler loop — owned by the host process, never by an Agent.
//!
//! Event-driven, not polling: sleep until the owner's earliest `next_run_at`,
//! woken early by CRUD (`Notify`) or shutdown (`CancellationToken`,
//! level-triggered like the manager event loop). With zero tasks the loop
//! parks on the notify/deadline futures with no timer registered — a parked
//! tokio task is the entire idle cost (RFC §5.1).
//!
//! Shutdown discipline (RFC §5.3): on cancellation the loop stops claiming;
//! an in-flight delivery (a channel send, milliseconds) finishes, retry
//! backoffs are interrupted, and any run left `claimed` is resolved by
//! [`ScheduleStore::reconcile_claimed`] at the next startup.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::store::{ClaimedFire, Owner, RunStatus, ScheduledTask, ScheduleStore};
use super::{MissedPolicy, ScheduleSettings};
use crate::error::Result;

/// Details of one fire, passed to the delivery implementation.
#[derive(Debug, Clone)]
pub struct FireInfo {
    /// The occurrence instant this fire stands for.
    pub scheduled_at: DateTime<Utc>,
    /// Actual delivery attempt time.
    pub fired_at: DateTime<Utc>,
    /// True when this is a downtime catch-up fire.
    pub catch_up: bool,
}

/// Frontend-specific delivery terminal. Implementations hand the wrapped
/// prompt to the owning session's input channel (Bot: `inject_message`;
/// GUI/TUI/Web: `FrontendMessage`). Success means **handed to the channel**,
/// not "the LLM turn finished" — turn outcomes are the Agent's concern.
#[async_trait]
pub trait ScheduleDelivery: Send + Sync {
    async fn deliver(&self, task: &ScheduledTask, fire: &FireInfo) -> Result<()>;
}

/// Retry backoffs between delivery attempts (seconds), capped at the last.
const RETRY_BACKOFF_SECS: [u64; 3] = [5, 30, 120];

/// Scheduler entry point.
pub struct Scheduler;

impl Scheduler {
    /// Spawn the loop. Returns immediately; the task ends when `shutdown`
    /// is cancelled. The host should stop the scheduler **before** draining
    /// agent sessions (RFC §5.3 ordering).
    pub fn spawn(
        store: ScheduleStore,
        owner: Owner,
        settings: ScheduleSettings,
        wake: Arc<Notify>,
        delivery: Arc<dyn ScheduleDelivery>,
        shutdown: CancellationToken,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            run_loop(store, owner, settings, wake, delivery, shutdown).await;
        })
    }
}

async fn run_loop(
    store: ScheduleStore,
    owner: Owner,
    settings: ScheduleSettings,
    wake: Arc<Notify>,
    delivery: Arc<dyn ScheduleDelivery>,
    shutdown: CancellationToken,
) {
    // ── Startup reconciliation: downtime missed occurrences + crash leftovers ──
    let now = Utc::now();
    match settings.missed_policy {
        MissedPolicy::Latest => {
            match store.reconcile_claimed(&owner, MissedPolicy::Latest) {
                Ok(fires) => {
                    for f in fires {
                        deliver_with_retry(&store, &delivery, &f, &settings, &shutdown).await;
                    }
                }
                Err(e) => tracing::error!("schedule reconcile_claimed failed: {}", e),
            }
            match store.claim_due(&owner, now, true) {
                Ok(fires) => {
                    for f in fires {
                        deliver_with_retry(&store, &delivery, &f, &settings, &shutdown).await;
                    }
                }
                Err(e) => tracing::error!("schedule catch-up claim failed: {}", e),
            }
        }
        MissedPolicy::Skip => {
            if let Err(e) = store.advance_past(&owner, now) {
                tracing::error!("schedule advance_past failed: {}", e);
            }
            if let Err(e) = store.reconcile_claimed(&owner, MissedPolicy::Skip) {
                tracing::error!("schedule reconcile_claimed(skip) failed: {}", e);
            }
        }
    }

    // ── Main loop ──
    loop {
        if shutdown.is_cancelled() {
            break;
        }
        let deadline = match store.min_next_run(&owner) {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("schedule min_next_run failed: {}", e);
                None
            }
        };
        let sleep = async {
            match deadline {
                Some(dt) => {
                    let dur = (dt - Utc::now()).to_std().unwrap_or(Duration::ZERO);
                    tokio::time::sleep(dur).await;
                }
                // No tasks: park until woken. No timer registered.
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = wake.notified() => {}
            _ = sleep => {}
        }
        if shutdown.is_cancelled() {
            break;
        }

        // Re-check against the real clock after waking (sleep duration is
        // never trusted — clock jumps / host suspend).
        let now = Utc::now();
        match store.claim_due(&owner, now, false) {
            Ok(fires) => {
                for f in fires {
                    if shutdown.is_cancelled() {
                        // Leave the run 'claimed' for next-startup reconciliation.
                        break;
                    }
                    deliver_with_retry(&store, &delivery, &f, &settings, &shutdown).await;
                    if let Err(e) = store.prune_runs(&f.task.id, 100) {
                        tracing::warn!("schedule prune_runs failed: {}", e);
                    }
                }
            }
            Err(e) => tracing::error!("schedule claim_due failed: {}", e),
        }
    }
    tracing::info!("Scheduler stopped (owner={}/{})", owner.source, owner.namespace);
}

/// Deliver one claimed fire with bounded retries; finalize the run row.
/// Shutdown interrupts backoffs and leaves the row `claimed` (reconciled at
/// next startup) rather than recording a spurious failure.
async fn deliver_with_retry(
    store: &ScheduleStore,
    delivery: &Arc<dyn ScheduleDelivery>,
    fire: &ClaimedFire,
    settings: &ScheduleSettings,
    shutdown: &CancellationToken,
) {
    let info = FireInfo {
        scheduled_at: fire.scheduled_at,
        fired_at: Utc::now(),
        catch_up: fire.catch_up,
    };
    let mut attempt: u32 = 0;
    loop {
        match delivery.deliver(&fire.task, &info).await {
            Ok(()) => {
                if let Err(e) = store.finish_run(fire.run_id, RunStatus::Delivered, None) {
                    tracing::error!("schedule finish_run(delivered) failed: {}", e);
                }
                tracing::info!(
                    "schedule fired: task={} title='{}' catch_up={}",
                    fire.task.id, fire.task.title, fire.catch_up
                );
                return;
            }
            Err(e) => {
                if attempt >= settings.delivery_retries {
                    tracing::error!(
                        "schedule delivery failed after {} attempt(s): task={} err={}",
                        attempt + 1, fire.task.id, e
                    );
                    if let Err(fe) =
                        store.finish_run(fire.run_id, RunStatus::Failed, Some(&e.to_string()))
                    {
                        tracing::error!("schedule finish_run(failed) failed: {}", fe);
                    }
                    return;
                }
                let backoff = RETRY_BACKOFF_SECS
                    [(attempt as usize).min(RETRY_BACKOFF_SECS.len() - 1)];
                attempt += 1;
                tracing::warn!(
                    "schedule delivery failed (attempt {}), retrying in {}s: task={} err={}",
                    attempt, backoff, fire.task.id, e
                );
                tokio::select! {
                    _ = shutdown.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::store::{TargetSpec, TaskStatus};
    use crate::schedule::trigger::ScheduleSpec;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingDelivery {
        calls: Mutex<Vec<(String, bool)>>,
        fail: bool,
    }

    #[async_trait]
    impl ScheduleDelivery for RecordingDelivery {
        async fn deliver(&self, task: &ScheduledTask, fire: &FireInfo) -> Result<()> {
            self.calls.lock().unwrap().push((task.id.clone(), fire.catch_up));
            if self.fail {
                Err(crate::error::AgentError::InternalError("boom".into()))
            } else {
                Ok(())
            }
        }
    }

    fn due_task(id: &str, at: DateTime<Utc>) -> ScheduledTask {
        ScheduledTask {
            id: id.into(),
            title: id.into(),
            prompt: "p".into(),
            schedule: ScheduleSpec::Once { at: crate::schedule::fmt_ts(at) },
            timezone: String::new(),
            target: TargetSpec::ThisSession { session_id: "s".into(), chat_id: None },
            source: "test".into(),
            namespace: String::new(),
            status: TaskStatus::Active,
            max_runs: None,
            run_count: 0,
            next_run_at: Some(at),
            last_run_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn owner() -> Owner {
        Owner { source: "test".into(), namespace: String::new() }
    }

    #[tokio::test]
    async fn fires_due_task_and_records_delivered() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let wake = Arc::new(Notify::new());
        let delivery = Arc::new(RecordingDelivery::default());
        let shutdown = CancellationToken::new();
        let handle = Scheduler::spawn(
            store.clone(),
            owner(),
            ScheduleSettings { enabled: true, delivery_retries: 0, ..Default::default() },
            wake.clone(),
            delivery.clone(),
            shutdown.clone(),
        );
        // Let startup reconciliation pass with an empty store, then insert a
        // due task and wake the loop — deterministic non-catch-up path.
        tokio::time::sleep(Duration::from_millis(50)).await;
        store.insert(&due_task("t1", Utc::now())).unwrap();
        wake.notify_one();
        tokio::time::sleep(Duration::from_millis(300)).await;
        shutdown.cancel();
        handle.await.unwrap();
        assert_eq!(*delivery.calls.lock().unwrap(), vec![("t1".to_string(), false)]);
        assert_eq!(store.runs_for("t1", 10).unwrap()[0].status, "delivered");
        assert_eq!(store.get_scoped("t1", &crate::schedule::TaskScope {
            source: "test".into(), namespace: String::new(), chat_id: None,
        }).unwrap().unwrap().status, TaskStatus::Completed);
    }

    #[tokio::test]
    async fn startup_catch_up_fires_overdue_once() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let at = Utc::now() - chrono::TimeDelta::hours(5);
        store.insert(&due_task("t1", at)).unwrap();
        let delivery = Arc::new(RecordingDelivery::default());
        let shutdown = CancellationToken::new();
        let handle = Scheduler::spawn(
            store.clone(),
            owner(),
            ScheduleSettings { enabled: true, ..Default::default() },
            Arc::new(Notify::new()),
            delivery.clone(),
            shutdown.clone(),
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        shutdown.cancel();
        handle.await.unwrap();
        assert_eq!(*delivery.calls.lock().unwrap(), vec![("t1".to_string(), true)]);
        let runs = store.runs_for("t1", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert!(runs[0].catch_up);
    }

    #[tokio::test]
    async fn skip_policy_does_not_fire_overdue() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let at = Utc::now() - chrono::TimeDelta::hours(5);
        store.insert(&due_task("t1", at)).unwrap();
        let delivery = Arc::new(RecordingDelivery::default());
        let shutdown = CancellationToken::new();
        let handle = Scheduler::spawn(
            store.clone(),
            owner(),
            ScheduleSettings { enabled: true, missed_policy: MissedPolicy::Skip, ..Default::default() },
            Arc::new(Notify::new()),
            delivery.clone(),
            shutdown.clone(),
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        shutdown.cancel();
        handle.await.unwrap();
        assert!(delivery.calls.lock().unwrap().is_empty());
        assert_eq!(store.runs_for("t1", 10).unwrap()[0].status, "skipped");
    }

    #[tokio::test]
    async fn delivery_failure_records_failed() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let at = Utc::now() + chrono::TimeDelta::milliseconds(30);
        store.insert(&due_task("t1", at)).unwrap();
        let delivery = Arc::new(RecordingDelivery { fail: true, ..Default::default() });
        let shutdown = CancellationToken::new();
        let handle = Scheduler::spawn(
            store.clone(),
            owner(),
            ScheduleSettings { enabled: true, delivery_retries: 0, ..Default::default() },
            Arc::new(Notify::new()),
            delivery.clone(),
            shutdown.clone(),
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        shutdown.cancel();
        handle.await.unwrap();
        let runs = store.runs_for("t1", 10).unwrap();
        assert_eq!(runs[0].status, "failed");
        assert!(runs[0].error.as_deref().unwrap().contains("boom"));
    }

    #[tokio::test]
    async fn wake_notify_picks_up_new_task_without_waiting() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let wake = Arc::new(Notify::new());
        let delivery = Arc::new(RecordingDelivery::default());
        let shutdown = CancellationToken::new();
        let handle = Scheduler::spawn(
            store.clone(),
            owner(),
            ScheduleSettings { enabled: true, ..Default::default() },
            wake.clone(),
            delivery.clone(),
            shutdown.clone(),
        );
        // Empty store: loop parks. Insert a task due immediately and wake it.
        tokio::time::sleep(Duration::from_millis(50)).await;
        store.insert(&due_task("t1", Utc::now())).unwrap();
        wake.notify_one();
        tokio::time::sleep(Duration::from_millis(200)).await;
        shutdown.cancel();
        handle.await.unwrap();
        assert_eq!(delivery.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn shutdown_stops_loop_promptly() {
        let store = ScheduleStore::open_in_memory().unwrap();
        let shutdown = CancellationToken::new();
        let handle = Scheduler::spawn(
            store,
            owner(),
            ScheduleSettings { enabled: true, ..Default::default() },
            Arc::new(Notify::new()),
            Arc::new(RecordingDelivery::default()),
            shutdown.clone(),
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("scheduler should stop within 2s of shutdown")
            .unwrap();
    }
}
