//! Native typing (busy-state) lifecycle for the WeChat adapter — **policy only**.
//!
//! Protocol calls are delegated to the SDK (`WeixinClient::send_typing` /
//! `cancel_typing`, weixin-agent ≥ 0.4); this module owns the per-chat state
//! cells, the refresh cadence, the failure breaker and shutdown cascading.
//! It contains no HTTP/protocol code — see `docs/robit-rfc-bot-typing-design.md`
//! §6 (W-D end state: protocol always lives in the SDK).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use weixin_agent::WeixinClient;

/// Transport seam over the SDK's client-level typing API.
///
/// Exists so the state machine below is testable offline; the production
/// implementation ([`SdkTyping`]) is pure delegation.
#[async_trait]
pub trait TypingTransport: Send + Sync {
    /// Light the typing indicator for a peer.
    async fn typing(&self, peer: &str, context_token: Option<&str>) -> weixin_agent::Result<()>;
    /// Cancel the typing indicator for a peer.
    async fn cancel(&self, peer: &str, context_token: Option<&str>) -> weixin_agent::Result<()>;
}

/// Production transport: delegates to the SDK client, no protocol code.
pub struct SdkTyping(pub Arc<WeixinClient>);

#[async_trait]
impl TypingTransport for SdkTyping {
    async fn typing(&self, peer: &str, context_token: Option<&str>) -> weixin_agent::Result<()> {
        self.0.send_typing(peer, context_token).await
    }
    async fn cancel(&self, peer: &str, context_token: Option<&str>) -> weixin_agent::Result<()> {
        self.0.cancel_typing(peer, context_token).await
    }
}

/// Consecutive send failures before the refresh loop gives up (degrades to
/// no typing; the next `set(true)` restarts it).
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// One lit chat: its refresh loop and the loop's cancellation token.
struct TypingCell {
    task: JoinHandle<()>,
    cancel: CancellationToken,
}

/// Per-chat typing state machine.
///
/// Contract (mirrors [`robit_chatbot::adapter::PlatformAdapter::set_typing`]):
/// - [`Self::set`] returns promptly; all HTTP happens inside spawned tasks.
/// - Idempotent: repeated `true` with a live loop is a no-op; `false` with
///   no cell is a no-op. A loop that died (failure breaker) is restarted by
///   the next `true`.
/// - Best-effort: transport failures are logged, never propagated.
/// - The context token is captured at transition time; refresh reuses it.
///   (Replies always resolve the freshest token via the adapter's own path;
///   a mid-turn token change only happens on a new inbound message, which
///   starts a new turn and re-lights anyway.)
pub struct TypingController {
    transport: Arc<dyn TypingTransport>,
    refresh: Duration,
    shutdown: CancellationToken,
    cells: Mutex<HashMap<String, TypingCell>>,
}

impl TypingController {
    /// Create a controller. `refresh` = cadence between typing re-sends while
    /// lit; [`Duration::ZERO`] means "send once, hold until cancelled".
    /// `shutdown` is the adapter's token — cancelling it stops every loop.
    pub fn new(
        transport: Arc<dyn TypingTransport>,
        refresh: Duration,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            transport,
            refresh,
            shutdown,
            cells: Mutex::new(HashMap::new()),
        }
    }

    /// Transition a chat's typing state. See the type-level contract.
    pub async fn set(&self, peer: &str, active: bool, context_token: Option<String>) {
        let mut cells = self.cells.lock().await;
        if active {
            if let Some(cell) = cells.get(peer) {
                if !cell.task.is_finished() {
                    return; // already lit, loop alive
                }
                // Loop died (failure breaker) — fall through to restart.
                cells.remove(peer);
            }
            let cancel = self.shutdown.child_token();
            let task = Self::spawn_loop(
                Arc::clone(&self.transport),
                peer.to_string(),
                context_token,
                self.refresh,
                cancel.clone(),
            );
            cells.insert(peer.to_string(), TypingCell { task, cancel });
        } else if let Some(cell) = cells.remove(peer) {
            cell.cancel.cancel();
            // Best-effort one-shot Cancel; never blocks the caller.
            let transport = Arc::clone(&self.transport);
            let peer = peer.to_string();
            tokio::spawn(async move {
                if let Err(e) = transport.cancel(&peer, context_token.as_deref()).await {
                    debug!("WeChat typing cancel failed for {}: {}", peer, e);
                }
            });
        }
    }

    /// The refresh loop: send immediately, then re-send every `refresh` until
    /// cancelled. Degrades (exits) after [`MAX_CONSECUTIVE_FAILURES`]; in
    /// no-refresh mode a single failure exits immediately.
    fn spawn_loop(
        transport: Arc<dyn TypingTransport>,
        peer: String,
        token: Option<String>,
        refresh: Duration,
        cancel: CancellationToken,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut failures = 0u32;
            loop {
                match transport.typing(&peer, token.as_deref()).await {
                    Ok(()) => failures = 0,
                    Err(e) => {
                        failures += 1;
                        if failures >= MAX_CONSECUTIVE_FAILURES {
                            warn!(
                                "WeChat typing for {} degraded after {} consecutive failures: {}; \
                                 re-lights on the next turn",
                                peer, failures, e
                            );
                            break;
                        }
                        warn!(
                            "WeChat typing send failed for {} ({}/{}): {}",
                            peer, failures, MAX_CONSECUTIVE_FAILURES, e
                        );
                        if refresh.is_zero() {
                            // No-refresh mode is single-attempt by definition.
                            break;
                        }
                    }
                }
                if refresh.is_zero() {
                    // Sent once: hold the cell open until cancelled so a
                    // repeated `true` stays a no-op.
                    cancel.cancelled().await;
                    break;
                }
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(refresh) => {}
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Recording transport; optionally fails the first N typing calls.
    struct MockTransport {
        calls: StdMutex<Vec<(String, bool)>>,
        fail_first: StdMutex<u32>,
    }

    impl MockTransport {
        fn new() -> Arc<Self> {
            Self::failing(0)
        }
        fn failing(n: u32) -> Arc<Self> {
            Arc::new(Self {
                calls: StdMutex::new(Vec::new()),
                fail_first: StdMutex::new(n),
            })
        }
        fn calls(&self) -> Vec<(String, bool)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl TypingTransport for MockTransport {
        async fn typing(&self, peer: &str, _token: Option<&str>) -> weixin_agent::Result<()> {
            self.calls.lock().unwrap().push((peer.to_string(), true));
            let mut n = self.fail_first.lock().unwrap();
            if *n > 0 {
                *n -= 1;
                return Err(weixin_agent::Error::Api {
                    errcode: -1,
                    errmsg: "mock failure".into(),
                });
            }
            Ok(())
        }
        async fn cancel(&self, peer: &str, _token: Option<&str>) -> weixin_agent::Result<()> {
            self.calls.lock().unwrap().push((peer.to_string(), false));
            Ok(())
        }
    }

    /// Let spawned tasks run to their next await point.
    async fn flush() {
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
    }

    fn controller(
        t: Arc<MockTransport>,
        refresh: Duration,
    ) -> (TypingController, CancellationToken) {
        let shutdown = CancellationToken::new();
        (
            TypingController::new(t, refresh, shutdown.clone()),
            shutdown,
        )
    }

    #[tokio::test]
    async fn light_then_cancel() {
        let t = MockTransport::new();
        let (c, _s) = controller(t.clone(), Duration::from_secs(5));
        c.set("u1", true, None).await;
        flush().await;
        c.set("u1", false, None).await;
        flush().await;
        assert_eq!(
            t.calls(),
            vec![("u1".to_string(), true), ("u1".to_string(), false)]
        );
    }

    #[tokio::test]
    async fn repeated_true_is_idempotent() {
        let t = MockTransport::new();
        let (c, _s) = controller(t.clone(), Duration::from_secs(5));
        for _ in 0..3 {
            c.set("u1", true, None).await;
            flush().await;
        }
        assert_eq!(t.calls(), vec![("u1".to_string(), true)]);
    }

    #[tokio::test]
    async fn false_without_cell_is_noop() {
        let t = MockTransport::new();
        let (c, _s) = controller(t.clone(), Duration::from_secs(5));
        c.set("u1", false, None).await;
        c.set("u1", false, None).await;
        flush().await;
        assert!(t.calls().is_empty());
    }

    #[tokio::test]
    async fn relight_after_cancel_rebuilds_the_loop() {
        let t = MockTransport::new();
        let (c, _s) = controller(t.clone(), Duration::from_secs(5));
        c.set("u1", true, None).await;
        flush().await;
        c.set("u1", false, None).await;
        flush().await;
        c.set("u1", true, None).await;
        flush().await;
        assert_eq!(
            t.calls(),
            vec![
                ("u1".to_string(), true),
                ("u1".to_string(), false),
                ("u1".to_string(), true),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_repeats_while_lit() {
        let t = MockTransport::new();
        let (c, _s) = controller(t.clone(), Duration::from_secs(5));
        c.set("u1", true, None).await;
        flush().await;
        assert_eq!(t.calls().len(), 1);
        tokio::time::advance(Duration::from_secs(5)).await;
        flush().await;
        assert_eq!(t.calls().len(), 2);
        tokio::time::advance(Duration::from_secs(5)).await;
        flush().await;
        assert_eq!(t.calls().len(), 3);
        c.set("u1", false, None).await;
        flush().await;
        assert_eq!(t.calls().last(), Some(&("u1".to_string(), false)));
        // No further refresh after cancel.
        tokio::time::advance(Duration::from_secs(30)).await;
        flush().await;
        assert_eq!(t.calls().len(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn zero_refresh_sends_once_and_holds() {
        let t = MockTransport::new();
        let (c, _s) = controller(t.clone(), Duration::ZERO);
        c.set("u1", true, None).await;
        flush().await;
        tokio::time::advance(Duration::from_secs(60)).await;
        flush().await;
        assert_eq!(t.calls(), vec![("u1".to_string(), true)]);
        // Still idempotent while held.
        c.set("u1", true, None).await;
        flush().await;
        assert_eq!(t.calls().len(), 1);
        c.set("u1", false, None).await;
        flush().await;
        assert_eq!(t.calls().last(), Some(&("u1".to_string(), false)));
    }

    #[tokio::test]
    async fn breaker_degrades_then_relights() {
        // No-refresh mode: each light is a single attempt, so the failure
        // sequence is fully deterministic (no timing dependence).
        let t = MockTransport::failing(3);
        let (c, _s) = controller(t.clone(), Duration::ZERO);
        for expected in 1..=3 {
            c.set("u1", true, None).await;
            flush().await;
            assert_eq!(t.calls().len(), expected, "attempt {expected} must fail once");
        }
        // Fourth light: failures exhausted, the send succeeds and holds.
        c.set("u1", true, None).await;
        flush().await;
        assert_eq!(t.calls().len(), 4);
        c.set("u1", true, None).await; // held → no-op
        flush().await;
        assert_eq!(t.calls().len(), 4);
        c.set("u1", false, None).await;
        flush().await;
        assert_eq!(t.calls().last(), Some(&("u1".to_string(), false)));
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_cascades_to_every_loop() {
        let t = MockTransport::new();
        let (c, shutdown) = controller(t.clone(), Duration::from_secs(5));
        c.set("u1", true, None).await;
        c.set("u2", true, None).await;
        flush().await;
        assert_eq!(t.calls().len(), 2);
        shutdown.cancel();
        tokio::time::advance(Duration::from_secs(30)).await;
        flush().await;
        // Both loops exited; no refresh after shutdown.
        assert_eq!(t.calls().len(), 2);
    }

    #[tokio::test]
    async fn chats_are_independent() {
        let t = MockTransport::new();
        let (c, _s) = controller(t.clone(), Duration::from_secs(5));
        c.set("u1", true, None).await;
        c.set("u2", true, None).await;
        flush().await;
        c.set("u1", false, None).await;
        flush().await;
        assert_eq!(
            t.calls(),
            vec![
                ("u1".to_string(), true),
                ("u2".to_string(), true),
                ("u1".to_string(), false),
            ]
        );
        // u2 still lit: repeated true is a no-op.
        c.set("u2", true, None).await;
        flush().await;
        assert_eq!(t.calls().len(), 3);
    }
}
