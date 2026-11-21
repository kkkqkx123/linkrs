use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use gpui_kit::{App, Global, Task};
use linkrs_client::{ClientError, SessionManager};

/// Maximum silent re-login attempts before the client drops to the sign-in
/// view. A small budget prevents a 401 storm from looping on bad credentials.
pub(crate) const MAX_RECONNECT_ATTEMPTS: u32 = 3;

/// Base delay before the first retry; doubles with each consecutive failure
/// and is capped so a stale password cannot busy-loop the server.
const RECONNECT_BACKOFF_BASE_MS: u64 = 1000;
const RECONNECT_BACKOFF_CAP_MS: u64 = 8000;

/// Exponential backoff for the `failures`-th consecutive failure, in
/// milliseconds: 1s, 2s, 4s, 8s (capped). `saturating_sub` also maps 0 to the
/// base delay, keeping the function total.
fn backoff_ms(failures: u32) -> u64 {
    let shift = failures.saturating_sub(1).min(3);
    RECONNECT_BACKOFF_BASE_MS
        .saturating_mul(1u64 << shift)
        .min(RECONNECT_BACKOFF_CAP_MS)
}

/// Coarse wall-clock milliseconds for the cooldown gate. Precision beyond a
/// millisecond is irrelevant here; a backwards system clock only delays a
/// retry, never causes a storm.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Shared, lock-free state that coordinates automatic re-login across
/// concurrent request failures. The atomics are also touched from the spawned
/// reconnect task, so the state is owned by an `Arc`.
#[derive(Default)]
pub struct ReconnectState {
    in_flight: AtomicBool,
    failures: AtomicU32,
    /// Monotonic millisecond timestamp before which a new attempt is refused.
    cooldown_until_ms: AtomicU64,
}

impl ReconnectState {
    /// Claim the single re-login slot. Returns false when one is already
    /// running, or while the cooldown from a recent failure is still active,
    /// so a burst of failing requests cannot trigger a login storm.
    pub fn try_start(&self) -> bool {
        if now_ms() < self.cooldown_until_ms.load(Ordering::SeqCst) {
            return false;
        }
        self.in_flight
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Clear the slot, the failure streak and the cooldown after a successful
    /// login, or when returning to the sign-in view.
    pub fn reset(&self) {
        self.failures.store(0, Ordering::SeqCst);
        self.cooldown_until_ms.store(0, Ordering::SeqCst);
        self.in_flight.store(false, Ordering::SeqCst);
    }

    /// Release the slot without touching the failure streak, used when no
    /// remembered credential is available to attempt a silent login.
    pub fn abort(&self) {
        self.in_flight.store(false, Ordering::SeqCst);
    }

    /// Record a failed attempt, arm the exponential cooldown and release the
    /// slot. Returns true when the budget is exhausted and the caller should
    /// drop to the sign-in view.
    pub fn finish_failure(&self) -> bool {
        let attempts = self.failures.fetch_add(1, Ordering::SeqCst) + 1;
        self.cooldown_until_ms.store(
            now_ms().saturating_add(backoff_ms(attempts)),
            Ordering::SeqCst,
        );
        self.in_flight.store(false, Ordering::SeqCst);
        attempts >= MAX_RECONNECT_ATTEMPTS
    }
}

/// Process-wide holder of the client session. The `SessionManager` is shared
/// with tokio-side request tasks through a `tokio::sync::Mutex`; UI state
/// mirrors only snapshots, so the UI thread never locks it.
pub struct SessionStore {
    manager: Arc<tokio::sync::Mutex<SessionManager>>,
    /// Keeps the background health-check loop alive for the whole app run.
    _heartbeat: Option<Task<()>>,
    /// Lock-free coordinator for automatic re-login.
    reconnect: Arc<ReconnectState>,
}

impl Global for SessionStore {}

impl SessionStore {
    pub fn new(host: &str, port: u16) -> Result<Self, ClientError> {
        Ok(Self {
            manager: Arc::new(tokio::sync::Mutex::new(SessionManager::new_http(
                host, port,
            )?)),
            _heartbeat: None,
            reconnect: Arc::new(ReconnectState::default()),
        })
    }

    pub fn init(store: Self, cx: &mut App) {
        cx.set_global(store);
    }

    pub fn manager(cx: &App) -> Arc<tokio::sync::Mutex<SessionManager>> {
        cx.global::<SessionStore>().manager.clone()
    }

    /// Handle to the shared re-login coordinator.
    pub fn reconnect(cx: &App) -> Arc<ReconnectState> {
        cx.global::<SessionStore>().reconnect.clone()
    }

    pub fn set_heartbeat(&mut self, task: Task<()>) {
        self._heartbeat = Some(task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_flight_blocks_a_second_attempt() {
        let state = ReconnectState::default();
        assert!(state.try_start());
        assert!(!state.try_start());
        state.reset();
        assert!(state.try_start());
    }

    #[test]
    fn failure_budget_drops_to_login_at_the_limit() {
        let state = ReconnectState::default();
        for _ in 1..MAX_RECONNECT_ATTEMPTS {
            assert!(state.try_start());
            assert!(!state.finish_failure());
            // Simulate the cooldown elapsing between user-driven retries.
            state.cooldown_until_ms.store(0, Ordering::SeqCst);
        }
        assert!(state.try_start());
        assert!(state.finish_failure());
    }

    #[test]
    fn abort_releases_the_slot_without_counting_a_failure() {
        let state = ReconnectState::default();
        assert!(state.try_start());
        state.abort();
        assert!(state.try_start());
        assert!(!state.finish_failure());
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        assert_eq!(backoff_ms(0), 1_000);
        assert_eq!(backoff_ms(1), 1_000);
        assert_eq!(backoff_ms(2), 2_000);
        assert_eq!(backoff_ms(3), 4_000);
        assert_eq!(backoff_ms(4), 8_000);
        assert_eq!(backoff_ms(9), 8_000);
    }

    #[test]
    fn cooldown_blocks_retry_until_it_elapses() {
        let state = ReconnectState::default();
        assert!(state.try_start());
        assert!(!state.finish_failure(), "first failure keeps the budget");
        assert!(!state.try_start(), "retry inside cooldown must be refused");
        state.cooldown_until_ms.store(0, Ordering::SeqCst);
        assert!(state.try_start(), "retry after cooldown must be allowed");
    }

    #[test]
    fn reset_clears_cooldown_and_failure_streak() {
        let state = ReconnectState::default();
        assert!(state.try_start());
        assert!(!state.finish_failure(), "first failure keeps the budget");
        state.reset();
        assert!(state.try_start());
        assert!(!state.finish_failure());
    }
}
