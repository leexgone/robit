//! Outbound send pacing for Bot platforms (platform-agnostic).
//!
//! Some IM platforms apply a server-side anti-flood limit: a burst of sends
//! makes *every* outbound call fail for minutes, and the penalty is account
//! level — it survives process restarts. (WeChat iLink: `ret=-2`,
//! `errmsg="prepare failed"`; other platforms express the same thing as HTTP
//! 429 + `Retry-After` or a vendor code.)
//!
//! This module is the platform-agnostic half of the defense: a per-instance
//! send budget (minimum spacing + sliding window), an advisory cooldown after
//! the platform reports throttling, and a persisted snapshot so a restart
//! cannot replay a burst.
//!
//! Platform knowledge stays in the adapter: it decides *when* the platform
//! throttled, how long to cool down, and where the snapshot lives. This module
//! only knows about time, and never inspects platform error codes.
//!
//! One limiter instance owns one quota domain. WeChat is limited per bot
//! account, so one instance per process/adapter is enough; a platform with
//! per-chat limits would hold one limiter per chat key instead — the module
//! itself is indifferent.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use robit_ai::config::RateLimitSection;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// Spacing between two consecutive sends (anti-clumping).
pub const DEFAULT_MIN_INTERVAL: Duration = Duration::from_millis(2_000);
/// Send budget per window.
pub const DEFAULT_WINDOW_SENDS: u32 = 10;
/// Sliding window length, aligned with the SDK's own safe point
/// (10 sends within 170s was observed to be safe; a short margin is added).
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(180);
/// Advisory cooldown after the platform reports throttling ("self-heals in
/// minutes").
pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(120);
/// Upper bound on how long a must-deliver send may queue (= one full window,
/// so a saturated window cannot turn into a hard failure).
pub const DEFAULT_MAX_WAIT: Duration = Duration::from_secs(180);

// Clamp ranges: values outside are clamped (with a warning) so no configuration
// can produce an unbounded wait or a zero-capacity (deadlocking) budget.
const MIN_INTERVAL_MAX_MS: u64 = 60_000;
const WINDOW_SENDS_MAX: u32 = 1_000;
const WINDOW_SECS_MAX: u64 = 3_600;
const COOLDOWN_SECS_MAX: u64 = 3_600;
const MAX_WAIT_MAX_MS: u64 = 600_000;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Resolved send budget. Built from [`RateLimitSection`] (config.toml) or from
/// [`ThrottleConfig::default`] (embedding hosts, no TOML involved).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThrottleConfig {
    /// Master switch. `false` restores the pre-limiter behavior entirely.
    pub enabled: bool,
    /// Minimum spacing between two sends; zero disables spacing.
    pub min_interval: Duration,
    /// How many sends are allowed inside `window_len`.
    pub window_sends: u32,
    /// Sliding window length.
    pub window_len: Duration,
    /// Cooldown applied when the platform reports throttling.
    pub cooldown: Duration,
    /// Longest a [`WaitMode::Final`] send may queue before giving up.
    pub max_wait: Duration,
}

impl Default for ThrottleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_interval: DEFAULT_MIN_INTERVAL,
            window_sends: DEFAULT_WINDOW_SENDS,
            window_len: DEFAULT_WINDOW,
            cooldown: DEFAULT_COOLDOWN,
            max_wait: DEFAULT_MAX_WAIT,
        }
    }
}

impl ThrottleConfig {
    /// Resolve a `[channels.<x>_bot.rate_limit]` section into concrete values.
    ///
    /// Missing fields fall back to the defaults above; out-of-range values are
    /// clamped; zero counts/lengths (which would deadlock the budget) fall back
    /// to the defaults. Every adjustment is logged once at startup.
    pub fn from_section(section: &RateLimitSection) -> Self {
        let mut cfg = Self {
            enabled: section.enabled.unwrap_or(true),
            ..Self::default()
        };

        if let Some(ms) = section.min_interval_ms {
            cfg.min_interval = Duration::from_millis(clamp_u64(
                ms,
                0,
                MIN_INTERVAL_MAX_MS,
                "rate_limit.min_interval_ms",
            ));
        }

        match section.window_sends {
            Some(0) => tracing::warn!(
                "rate_limit.window_sends = 0 would block every send; using default {}",
                DEFAULT_WINDOW_SENDS
            ),
            Some(n) => {
                cfg.window_sends = clamp_u64(
                    u64::from(n),
                    1,
                    u64::from(WINDOW_SENDS_MAX),
                    "rate_limit.window_sends",
                ) as u32
            }
            None => {}
        }

        match section.window_secs {
            Some(0) => tracing::warn!(
                "rate_limit.window_secs = 0 is invalid; using default {}s",
                DEFAULT_WINDOW.as_secs()
            ),
            Some(secs) => {
                cfg.window_len = Duration::from_secs(clamp_u64(
                    secs,
                    1,
                    WINDOW_SECS_MAX,
                    "rate_limit.window_secs",
                ))
            }
            None => {}
        }

        if let Some(secs) = section.cooldown_secs {
            cfg.cooldown = Duration::from_secs(clamp_u64(
                secs,
                0,
                COOLDOWN_SECS_MAX,
                "rate_limit.cooldown_secs",
            ));
        }

        if let Some(ms) = section.max_wait_ms {
            cfg.max_wait =
                Duration::from_millis(clamp_u64(ms, 0, MAX_WAIT_MAX_MS, "rate_limit.max_wait_ms"));
        }

        cfg
    }

    fn min_interval_ms(&self) -> u64 {
        self.min_interval.as_millis() as u64
    }

    fn window_ms(&self) -> u64 {
        self.window_len.as_millis() as u64
    }

    fn max_wait_ms(&self) -> u64 {
        self.max_wait.as_millis() as u64
    }
}

fn clamp_u64(value: u64, lo: u64, hi: u64, key: &str) -> u64 {
    let clamped = value.clamp(lo, hi);
    if clamped != value {
        tracing::warn!(
            "{} = {} is out of range [{}..={}]; using {}",
            key,
            value,
            lo,
            hi,
            clamped
        );
    }
    clamped
}

// ---------------------------------------------------------------------------
// Wait policy and results
// ---------------------------------------------------------------------------

/// How a caller wants to wait for a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitMode {
    /// Must be delivered (final answers, host pushes, confirmation prompts):
    /// queue for a slot, bounded by [`ThrottleConfig::max_wait`].
    Final,
    /// Optional (progress hints, notices): never wait — fail immediately so a
    /// throttled burst cannot delay the messages that matter.
    BestEffort,
}

/// Why a reservation was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ThrottleError {
    /// The platform reported throttling; a cooldown is in effect.
    #[error("platform cooldown active; retry in {:?}", retry_after)]
    Cooldown { retry_after: Duration },
    /// Our own budget would require waiting longer than `max_wait`.
    #[error("send budget saturated; retry in {:?}", retry_after)]
    Saturated { retry_after: Duration },
    /// Shutdown happened while waiting for a slot.
    #[error("send throttling cancelled (shutdown)")]
    Cancelled,
}

impl ThrottleError {
    /// Suggested delay before the caller retries.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Cooldown { retry_after } | Self::Saturated { retry_after } => Some(*retry_after),
            Self::Cancelled => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Persisted snapshot
// ---------------------------------------------------------------------------

/// A limiter's state at a point in time, so a restart cannot replay a burst.
///
/// Platform adapters persist it wherever their own state lives (WeChat writes
/// it into `wechat-state.json`); this module never touches the filesystem.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThrottleState {
    /// Unix-ms timestamps of recent sends, oldest first.
    #[serde(default)]
    pub recent_ms: Vec<u64>,
    /// Unix ms at which the platform cooldown ends; `0` means none.
    #[serde(default)]
    pub cooldown_until_ms: u64,
}

// ---------------------------------------------------------------------------
// Limiter
// ---------------------------------------------------------------------------

struct Queue {
    /// Unix-ms timestamps of sends still inside the window, oldest first.
    recent: VecDeque<u64>,
}

/// Platform-agnostic send budget.
///
/// Clone-free by design: the owning adapter holds it in an `Arc` and shares it
/// between every outbound path (text, media, hints) so one quota domain is
/// enforced per instance.
pub struct SendLimiter {
    cfg: ThrottleConfig,
    queue: Mutex<Queue>,
    /// Unix ms; `fetch_max` keeps concurrent notifications monotonic.
    cooldown_until_ms: AtomicU64,
}

impl SendLimiter {
    pub fn new(cfg: ThrottleConfig) -> Self {
        Self {
            cfg,
            queue: Mutex::new(Queue {
                recent: VecDeque::new(),
            }),
            cooldown_until_ms: AtomicU64::new(0),
        }
    }

    pub fn config(&self) -> &ThrottleConfig {
        &self.cfg
    }

    /// Reserve one send slot.
    ///
    /// `Ok(())` means the caller may send now and the slot is already booked:
    /// a send that fails afterwards still counts (the server may have received
    /// it). `Err` means "do not send"; the variant tells the caller whether it
    /// was a platform cooldown, our own saturated budget, or a shutdown.
    pub async fn reserve(
        &self,
        mode: WaitMode,
        cancel: &CancellationToken,
    ) -> Result<(), ThrottleError> {
        if !self.cfg.enabled {
            return Ok(());
        }
        let deadline = now_ms().saturating_add(self.cfg.max_wait_ms());

        loop {
            let now = now_ms();

            // 1) Platform-imposed cooldown (set out of band by the adapter).
            let until = self.cooldown_until_ms.load(Ordering::Relaxed);
            if until > now {
                let wait = Duration::from_millis(until - now);
                if mode == WaitMode::BestEffort || !within_budget(wait, deadline, now) {
                    return Err(ThrottleError::Cooldown { retry_after: wait });
                }
                sleep_or_cancel(wait, cancel).await?;
                continue;
            }

            // 2) Our own budget: decide under the lock, then wait outside it so
            //    `snapshot` / `note_throttled` never queue behind a sleeper.
            let wait = {
                let mut queue = self.queue.lock().await;
                self.prune(&mut queue, now);

                if (queue.recent.len() as u32) < self.cfg.window_sends {
                    let next_at = match queue.recent.back() {
                        Some(last) => last.saturating_add(self.cfg.min_interval_ms()),
                        None => now,
                    };
                    if now >= next_at {
                        queue.recent.push_back(now);
                        return Ok(());
                    }
                    Duration::from_millis(next_at - now)
                } else {
                    // Window full: the oldest entry frees the next slot.
                    let freed_at = queue
                        .recent
                        .front()
                        .copied()
                        .unwrap_or(now)
                        .saturating_add(self.cfg.window_ms());
                    Duration::from_millis(freed_at.saturating_sub(now))
                }
            };

            if mode == WaitMode::BestEffort {
                return Err(ThrottleError::Saturated { retry_after: wait });
            }
            if !within_budget(wait, deadline, now) {
                return Err(ThrottleError::Saturated { retry_after: wait });
            }
            sleep_or_cancel(wait, cancel).await?;
        }
    }

    /// Apply a cooldown after the platform reported throttling.
    ///
    /// Returns `true` when this notification started a *fresh* cooldown (no
    /// cooldown was active), which adapters use to log the incident once
    /// instead of once per failed send.
    pub fn note_throttled(&self, cooldown: Duration) -> bool {
        if cooldown.is_zero() {
            return false;
        }
        let now = now_ms();
        let until = now.saturating_add(cooldown.as_millis() as u64);
        let previous = self.cooldown_until_ms.load(Ordering::Relaxed);
        let fresh = previous <= now;
        self.cooldown_until_ms.fetch_max(until, Ordering::Relaxed);
        fresh
    }

    /// Current state, for the owning adapter to persist.
    pub async fn snapshot(&self) -> ThrottleState {
        let queue = self.queue.lock().await;
        ThrottleState {
            recent_ms: queue.recent.iter().copied().collect(),
            cooldown_until_ms: self.cooldown_until_ms.load(Ordering::Relaxed),
        }
    }

    /// Restore a previously persisted snapshot (call once at startup).
    pub async fn restore(&self, state: &ThrottleState) {
        self.cooldown_until_ms
            .store(state.cooldown_until_ms, Ordering::Relaxed);
        let mut queue = self.queue.lock().await;
        queue.recent.clear();
        queue.recent.extend(state.recent_ms.iter().copied());
        let now = now_ms();
        self.prune(&mut queue, now);
    }

    fn prune(&self, queue: &mut Queue, now: u64) {
        let horizon = now.saturating_sub(self.cfg.window_ms());
        while let Some(&front) = queue.recent.front() {
            if front <= horizon {
                queue.recent.pop_front();
            } else {
                break;
            }
        }
        // Defensive: a restored/hand-edited snapshot cannot exceed the budget.
        while queue.recent.len() as u32 > self.cfg.window_sends {
            queue.recent.pop_front();
        }
    }
}

/// Whether `wait` fits in the remaining `max_wait` budget.
fn within_budget(wait: Duration, deadline_ms: u64, now_ms: u64) -> bool {
    let budget = deadline_ms.saturating_sub(now_ms);
    (wait.as_millis() as u64) <= budget
}

/// Sleep for `dur`, aborting as soon as `cancel` fires.
async fn sleep_or_cancel(dur: Duration, cancel: &CancellationToken) -> Result<(), ThrottleError> {
    if dur.is_zero() {
        return Ok(());
    }
    tokio::select! {
        _ = tokio::time::sleep(dur) => Ok(()),
        _ = cancel.cancelled() => Err(ThrottleError::Cancelled),
    }
}

/// Unix milliseconds. Wall-clock (not `Instant`) so snapshots survive restarts.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn cfg(
        min_interval_ms: u64,
        window_sends: u32,
        window_ms: u64,
        max_wait_ms: u64,
    ) -> ThrottleConfig {
        ThrottleConfig {
            enabled: true,
            min_interval: Duration::from_millis(min_interval_ms),
            window_sends,
            window_len: Duration::from_millis(window_ms),
            cooldown: Duration::from_millis(80),
            max_wait: Duration::from_millis(max_wait_ms),
        }
    }

    fn no_cancel() -> CancellationToken {
        CancellationToken::new()
    }

    #[tokio::test]
    async fn first_send_is_immediate() {
        let limiter = SendLimiter::new(cfg(2000, 10, 180_000, 180_000));
        let started = Instant::now();
        limiter
            .reserve(WaitMode::Final, &no_cancel())
            .await
            .unwrap();
        assert!(started.elapsed() < Duration::from_millis(20));
    }

    #[tokio::test]
    async fn min_interval_spaces_consecutive_sends() {
        let limiter = SendLimiter::new(cfg(60, 10, 300, 1000));
        limiter
            .reserve(WaitMode::Final, &no_cancel())
            .await
            .unwrap();

        let started = Instant::now();
        limiter
            .reserve(WaitMode::Final, &no_cancel())
            .await
            .unwrap();
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(45),
            "waited only {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn best_effort_never_waits() {
        let limiter = SendLimiter::new(cfg(60, 10, 300, 1000));
        limiter
            .reserve(WaitMode::Final, &no_cancel())
            .await
            .unwrap();

        let started = Instant::now();
        let err = limiter
            .reserve(WaitMode::BestEffort, &no_cancel())
            .await
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_millis(15));
        assert!(matches!(err, ThrottleError::Saturated { .. }));
        assert!(err.retry_after().is_some());
    }

    #[tokio::test]
    async fn window_cap_is_enforced() {
        // 3 sends per 250ms, no spacing: the 4th must wait for the window.
        let limiter = SendLimiter::new(cfg(0, 3, 250, 500));
        for _ in 0..3 {
            limiter
                .reserve(WaitMode::Final, &no_cancel())
                .await
                .unwrap();
        }
        let started = Instant::now();
        limiter
            .reserve(WaitMode::Final, &no_cancel())
            .await
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(150));
    }

    #[tokio::test]
    async fn max_wait_refuses_long_queues() {
        // Window needs ~250ms to free a slot, budget is only 40ms.
        let limiter = SendLimiter::new(cfg(0, 2, 250, 40));
        limiter
            .reserve(WaitMode::Final, &no_cancel())
            .await
            .unwrap();
        limiter
            .reserve(WaitMode::Final, &no_cancel())
            .await
            .unwrap();

        let err = limiter
            .reserve(WaitMode::Final, &no_cancel())
            .await
            .unwrap_err();
        match err {
            ThrottleError::Saturated { retry_after } => {
                assert!(retry_after >= Duration::from_millis(20));
            }
            other => panic!("expected Saturated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cooldown_blocks_final_and_fails_best_effort() {
        let limiter = SendLimiter::new(cfg(0, 10, 300, 1000));
        assert!(limiter.note_throttled(Duration::from_millis(60)));

        // Best-effort: refused immediately.
        let started = Instant::now();
        let err = limiter
            .reserve(WaitMode::BestEffort, &no_cancel())
            .await
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_millis(15));
        assert!(matches!(err, ThrottleError::Cooldown { .. }));

        // Must-deliver: waits the cooldown out, then sends.
        let started = Instant::now();
        limiter
            .reserve(WaitMode::Final, &no_cancel())
            .await
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(45));
    }

    #[tokio::test]
    async fn note_throttled_reports_only_fresh_cooldowns() {
        let limiter = SendLimiter::new(cfg(0, 10, 300, 1000));
        assert!(limiter.note_throttled(Duration::from_millis(60)));
        // A second report inside the same cooldown is not a fresh incident.
        assert!(!limiter.note_throttled(Duration::from_millis(60)));
        // Zero-length cooldown is a no-op.
        assert!(!limiter.note_throttled(Duration::ZERO));
    }

    #[tokio::test]
    async fn shutdown_cancels_a_queued_waiter() {
        let limiter = SendLimiter::new(cfg(200, 10, 300, 1000));
        limiter
            .reserve(WaitMode::Final, &no_cancel())
            .await
            .unwrap();

        let cancel = CancellationToken::new();
        let waiter = {
            let cancel = cancel.clone();
            async move { limiter.reserve(WaitMode::Final, &cancel).await }
        };
        let handle = tokio::spawn(waiter);
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();

        let started = Instant::now();
        let err = handle.await.unwrap().unwrap_err();
        assert!(matches!(err, ThrottleError::Cancelled));
        assert!(started.elapsed() < Duration::from_millis(150));
    }

    #[tokio::test]
    async fn disabled_limiter_never_blocks() {
        let mut config = cfg(2000, 1, 1000, 1000);
        config.enabled = false;
        let limiter = SendLimiter::new(config);
        for _ in 0..5 {
            limiter
                .reserve(WaitMode::Final, &no_cancel())
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn snapshot_restores_a_saturated_window() {
        let limiter = SendLimiter::new(cfg(0, 2, 400, 800));
        limiter
            .reserve(WaitMode::Final, &no_cancel())
            .await
            .unwrap();
        limiter
            .reserve(WaitMode::Final, &no_cancel())
            .await
            .unwrap();
        let snap = limiter.snapshot().await;
        assert_eq!(snap.recent_ms.len(), 2);

        let restored = SendLimiter::new(cfg(0, 2, 400, 800));
        restored.restore(&snap).await;
        let started = Instant::now();
        restored
            .reserve(WaitMode::Final, &no_cancel())
            .await
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(200));
    }

    #[tokio::test]
    async fn restore_prunes_stale_timestamps() {
        let limiter = SendLimiter::new(cfg(0, 3, 100, 500));
        let now = now_ms();
        let stale = ThrottleState {
            recent_ms: vec![now.saturating_sub(10_000), now.saturating_sub(5_000)],
            cooldown_until_ms: now.saturating_sub(1_000),
        };
        limiter.restore(&stale).await;
        // Everything was outside the window and the cooldown expired: immediate.
        let started = Instant::now();
        limiter
            .reserve(WaitMode::Final, &no_cancel())
            .await
            .unwrap();
        assert!(started.elapsed() < Duration::from_millis(20));
    }

    #[test]
    fn config_defaults_match_documented_values() {
        let cfg = ThrottleConfig::from_section(&RateLimitSection::default());
        assert_eq!(cfg, ThrottleConfig::default());
        assert!(cfg.enabled);
        assert_eq!(cfg.min_interval, Duration::from_millis(2_000));
        assert_eq!(cfg.window_sends, 10);
        assert_eq!(cfg.window_len, Duration::from_secs(180));
        assert_eq!(cfg.cooldown, Duration::from_secs(120));
        assert_eq!(cfg.max_wait, Duration::from_secs(180));
    }

    #[test]
    fn config_overrides_are_parsed() {
        let section: RateLimitSection = toml::from_str(
            r#"
            enabled = false
            min_interval_ms = 500
            window_sends = 4
            window_secs = 60
            cooldown_secs = 30
            max_wait_ms = 5000
            "#,
        )
        .unwrap();
        let cfg = ThrottleConfig::from_section(&section);
        assert!(!cfg.enabled);
        assert_eq!(cfg.min_interval, Duration::from_millis(500));
        assert_eq!(cfg.window_sends, 4);
        assert_eq!(cfg.window_len, Duration::from_secs(60));
        assert_eq!(cfg.cooldown, Duration::from_secs(30));
        assert_eq!(cfg.max_wait, Duration::from_millis(5_000));
    }

    #[test]
    fn config_zero_values_fall_back_to_defaults() {
        let section: RateLimitSection = toml::from_str(
            r#"
            window_sends = 0
            window_secs = 0
            "#,
        )
        .unwrap();
        let cfg = ThrottleConfig::from_section(&section);
        assert_eq!(cfg.window_sends, DEFAULT_WINDOW_SENDS);
        assert_eq!(cfg.window_len, DEFAULT_WINDOW);
    }

    #[test]
    fn config_out_of_range_values_are_clamped() {
        let section: RateLimitSection = toml::from_str(
            r#"
            min_interval_ms = 999999
            window_sends = 5000
            window_secs = 99999
            cooldown_secs = 99999
            max_wait_ms = 99999999
            "#,
        )
        .unwrap();
        let cfg = ThrottleConfig::from_section(&section);
        assert_eq!(cfg.min_interval, Duration::from_millis(MIN_INTERVAL_MAX_MS));
        assert_eq!(cfg.window_sends, WINDOW_SENDS_MAX);
        assert_eq!(cfg.window_len, Duration::from_secs(WINDOW_SECS_MAX));
        assert_eq!(cfg.cooldown, Duration::from_secs(COOLDOWN_SECS_MAX));
        assert_eq!(cfg.max_wait, Duration::from_millis(MAX_WAIT_MAX_MS));
    }

    #[test]
    fn state_json_is_backward_compatible() {
        // An older state file without the `outbound` fields must still parse.
        let state: ThrottleState = serde_json::from_str("{}").unwrap();
        assert!(state.recent_ms.is_empty());
        assert_eq!(state.cooldown_until_ms, 0);
    }
}
