//! LLM-call retry policy (`[app.retry]`) and per-call timeout guards.
//!
//! Until 2026-11-25 the `[app.retry]` config section was parsed but never
//! used, and LLM requests had no timeouts at any layer: a silently dropped
//! keep-alive connection or a stalled SSE stream blocked the session's agent
//! loop forever (the robit-qq "bot online but never replies" incident).
//!
//! The fix is layered:
//! - transport: `robit_ai::client` builds its reqwest client with connect /
//!   read-idle timeouts and connection-pool hygiene;
//! - call: [`RetrySettings::first_byte_timeout_secs`] bounds stream creation
//!   (connect + send + response headers) and
//!   [`RetrySettings::stream_idle_timeout_secs`] bounds the silence between
//!   two SSE chunks — both enforced in `Agent::run_one_step`;
//! - retry: transient failures (see [`robit_ai::LlmError::is_retryable`])
//!   retry with exponential backoff via [`retry_backoff`], but only while
//!   nothing has been streamed to the frontend yet, so a retry can never
//!   duplicate user-visible output. After the budget is exhausted the error
//!   surfaces through the normal `AgentEvent::Error` path, releasing the loop
//!   for queued messages.

use robit_ai::config::RetryConfig;
use robit_ai::LlmError;

/// Resolved retry + timeout settings for one Agent.
///
/// Defaults mirror the documented `[app.retry]` values; the two timeout
/// fields are agent-side guards on top of the transport-level reqwest
/// timeouts (which use fixed, conservative values in `robit-ai`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetrySettings {
    /// Retries after the initial attempt (default 3 → up to 4 attempts).
    pub max_retries: u32,
    /// Backoff before the first retry, milliseconds (default 1000).
    pub initial_backoff_ms: u64,
    /// Backoff cap, milliseconds (default 30000).
    pub max_backoff_ms: u64,
    /// Max seconds to wait for stream creation (connect + response headers)
    /// before failing with `LlmError::Timeout` (default 60).
    pub first_byte_timeout_secs: u64,
    /// Max seconds of silence between two SSE chunks before failing with
    /// `LlmError::Timeout` (default 90). Long generations are unaffected as
    /// long as chunks keep arriving.
    pub stream_idle_timeout_secs: u64,
}

impl Default for RetrySettings {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff_ms: 1000,
            max_backoff_ms: 30000,
            first_byte_timeout_secs: 60,
            stream_idle_timeout_secs: 90,
        }
    }
}

impl RetrySettings {
    /// Resolve `[app.retry]` over the defaults. Missing section or fields
    /// keep the documented default values.
    pub fn from_config(cfg: Option<&RetryConfig>) -> Self {
        let d = Self::default();
        match cfg {
            None => d,
            Some(c) => Self {
                max_retries: c.max_retries.unwrap_or(d.max_retries),
                initial_backoff_ms: c.initial_backoff_ms.unwrap_or(d.initial_backoff_ms),
                max_backoff_ms: c.max_backoff_ms.unwrap_or(d.max_backoff_ms),
                ..d
            },
        }
    }

    /// Exponential backoff for retry `attempt` (0-based):
    /// `initial * 2^attempt`, capped at `max_backoff_ms`. Overflow-safe.
    pub fn backoff_ms(&self, attempt: u32) -> u64 {
        let factor = 1u64.checked_shl(attempt.min(63)).unwrap_or(u64::MAX);
        self.initial_backoff_ms
            .saturating_mul(factor)
            .min(self.max_backoff_ms)
    }
}

/// Decide whether a failed LLM call should be retried.
///
/// Returns the backoff in milliseconds, or `None` to give up and surface the
/// error. A retry is allowed only when ALL hold:
/// - the error is transient ([`LlmError::is_retryable`]);
/// - the retry budget is not exhausted (`attempt < max_retries`);
/// - nothing has been streamed to the frontend yet (`emitted_delta == false`)
///   — retrying after partial output would duplicate the user-visible text.
pub fn retry_backoff(
    settings: &RetrySettings,
    err: &LlmError,
    attempt: u32,
    emitted_delta: bool,
) -> Option<u64> {
    if emitted_delta || attempt >= settings.max_retries || !err.is_retryable() {
        return None;
    }
    Some(settings.backoff_ms(attempt))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_documented_config() {
        let s = RetrySettings::default();
        assert_eq!(s.max_retries, 3);
        assert_eq!(s.initial_backoff_ms, 1000);
        assert_eq!(s.max_backoff_ms, 30000);
        assert_eq!(s.first_byte_timeout_secs, 60);
        assert_eq!(s.stream_idle_timeout_secs, 90);
        assert_eq!(RetrySettings::from_config(None), s);
    }

    #[test]
    fn from_config_overrides_only_provided_fields() {
        let cfg = RetryConfig {
            max_retries: Some(5),
            initial_backoff_ms: None,
            max_backoff_ms: Some(10_000),
        };
        let s = RetrySettings::from_config(Some(&cfg));
        assert_eq!(s.max_retries, 5);
        assert_eq!(s.initial_backoff_ms, 1000); // default kept
        assert_eq!(s.max_backoff_ms, 10_000);
        assert_eq!(s.first_byte_timeout_secs, 60); // agent-side default kept
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        let s = RetrySettings::default();
        assert_eq!(s.backoff_ms(0), 1000);
        assert_eq!(s.backoff_ms(1), 2000);
        assert_eq!(s.backoff_ms(2), 4000);
        assert_eq!(s.backoff_ms(4), 16000);
        assert_eq!(s.backoff_ms(5), 30000); // capped
        assert_eq!(s.backoff_ms(1000), 30000); // no overflow/panic
    }

    #[test]
    fn backoff_zero_initial_means_no_wait() {
        let s = RetrySettings {
            initial_backoff_ms: 0,
            ..Default::default()
        };
        assert_eq!(s.backoff_ms(3), 0);
    }

    #[test]
    fn retry_backoff_gates_on_budget_error_and_output() {
        let s = RetrySettings::default();
        let transient = LlmError::Timeout { detail: "x".into() };
        let permanent = LlmError::AuthenticationError;

        // Transient, budget left, nothing emitted → retry with backoff.
        assert_eq!(retry_backoff(&s, &transient, 0, false), Some(1000));
        assert_eq!(retry_backoff(&s, &transient, 2, false), Some(4000));
        // Budget exhausted.
        assert_eq!(retry_backoff(&s, &transient, 3, false), None);
        // Permanent error never retries.
        assert_eq!(retry_backoff(&s, &permanent, 0, false), None);
        // Partial output already shown → never retry (would duplicate text).
        assert_eq!(retry_backoff(&s, &transient, 0, true), None);
        // max_retries = 0 disables retrying entirely.
        let no_retry = RetrySettings { max_retries: 0, ..Default::default() };
        assert_eq!(retry_backoff(&no_retry, &transient, 0, false), None);
    }
}
