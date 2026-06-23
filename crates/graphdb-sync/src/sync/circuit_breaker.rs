//! Circuit Breaker for Remote Index Operations
//!
//! Implements the circuit breaker pattern to prevent cascading failures
//! when remote services (e.g., Qdrant) are unavailable.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tracing::{debug, warn};

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation - requests are allowed
    Closed,
    /// Failing - requests are blocked, waiting for recovery
    Open,
    /// Testing if service has recovered
    HalfOpen,
}

impl std::fmt::Display for CircuitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitState::Closed => write!(f, "closed"),
            CircuitState::Open => write!(f, "open"),
            CircuitState::HalfOpen => write!(f, "half-open"),
        }
    }
}

/// Circuit breaker configuration
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of failures before opening the circuit
    pub failure_threshold: u64,
    /// Time to wait before attempting recovery (transition to half-open)
    pub recovery_timeout: Duration,
    /// Number of successful requests in half-open state to close the circuit
    pub success_threshold: u64,
    /// Time window for counting failures
    pub failure_window: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            recovery_timeout: Duration::from_secs(30),
            success_threshold: 3,
            failure_window: Duration::from_secs(60),
        }
    }
}

impl CircuitBreakerConfig {
    pub fn new(
        failure_threshold: u64,
        recovery_timeout: Duration,
        success_threshold: u64,
        failure_window: Duration,
    ) -> Self {
        Self {
            failure_threshold,
            recovery_timeout,
            success_threshold,
            failure_window,
        }
    }

    pub fn with_failure_threshold(mut self, threshold: u64) -> Self {
        self.failure_threshold = threshold;
        self
    }

    pub fn with_recovery_timeout(mut self, timeout: Duration) -> Self {
        self.recovery_timeout = timeout;
        self
    }

    pub fn with_success_threshold(mut self, threshold: u64) -> Self {
        self.success_threshold = threshold;
        self
    }

    pub fn with_failure_window(mut self, window: Duration) -> Self {
        self.failure_window = window;
        self
    }
}

/// Internal state for tracking failures
#[derive(Debug)]
struct CircuitBreakerState {
    state: CircuitState,
    failure_count: u64,
    success_count: u64,
    last_failure_time: Option<Instant>,
    opened_at: Option<Instant>,
}

impl Default for CircuitBreakerState {
    fn default() -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            success_count: 0,
            last_failure_time: None,
            opened_at: None,
        }
    }
}

/// Circuit breaker for protecting remote service calls
#[derive(Debug)]
pub struct CircuitBreaker {
    name: String,
    config: CircuitBreakerConfig,
    state: RwLock<CircuitBreakerState>,
    total_requests: AtomicU64,
    total_failures: AtomicU64,
    total_successes: AtomicU64,
    total_rejected: AtomicU64,
}

impl CircuitBreaker {
    pub fn new(name: impl Into<String>, config: CircuitBreakerConfig) -> Self {
        Self {
            name: name.into(),
            config,
            state: RwLock::new(CircuitBreakerState::default()),
            total_requests: AtomicU64::new(0),
            total_failures: AtomicU64::new(0),
            total_successes: AtomicU64::new(0),
            total_rejected: AtomicU64::new(0),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Check if a request is allowed
    pub fn is_allowed(&self) -> bool {
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        let mut state = self.state.write();

        match state.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if let Some(opened_at) = state.opened_at {
                    if opened_at.elapsed() >= self.config.recovery_timeout {
                        debug!(
                            circuit_breaker = %self.name,
                            "Circuit breaker transitioning to half-open"
                        );
                        state.state = CircuitState::HalfOpen;
                        state.success_count = 0;
                        return true;
                    }
                }
                self.total_rejected.fetch_add(1, Ordering::Relaxed);
                debug!(
                    circuit_breaker = %self.name,
                    "Request rejected - circuit is open"
                );
                false
            }
            CircuitState::HalfOpen => true,
        }
    }

    /// Record a successful operation
    pub fn record_success(&self) {
        self.total_successes.fetch_add(1, Ordering::Relaxed);

        let mut state = self.state.write();

        match state.state {
            CircuitState::Closed => {
                self.reset_failure_count(&mut state);
            }
            CircuitState::HalfOpen => {
                state.success_count += 1;
                if state.success_count >= self.config.success_threshold {
                    debug!(
                        circuit_breaker = %self.name,
                        success_count = state.success_count,
                        "Circuit breaker closing after successful recovery"
                    );
                    state.state = CircuitState::Closed;
                    state.failure_count = 0;
                    state.opened_at = None;
                }
            }
            CircuitState::Open => {}
        }
    }

    /// Record a failed operation
    pub fn record_failure(&self) {
        self.total_failures.fetch_add(1, Ordering::Relaxed);

        let mut state = self.state.write();

        match state.state {
            CircuitState::Closed => {
                let now = Instant::now();

                if let Some(last_failure) = state.last_failure_time {
                    if now.duration_since(last_failure) > self.config.failure_window {
                        state.failure_count = 0;
                    }
                }

                state.failure_count += 1;
                state.last_failure_time = Some(now);

                if state.failure_count >= self.config.failure_threshold {
                    warn!(
                        circuit_breaker = %self.name,
                        failure_count = state.failure_count,
                        threshold = self.config.failure_threshold,
                        "Circuit breaker opening due to failures"
                    );
                    state.state = CircuitState::Open;
                    state.opened_at = Some(now);
                }
            }
            CircuitState::HalfOpen => {
                warn!(
                    circuit_breaker = %self.name,
                    "Circuit breaker reopening due to failure in half-open state"
                );
                state.state = CircuitState::Open;
                state.opened_at = Some(Instant::now());
                state.success_count = 0;
            }
            CircuitState::Open => {}
        }
    }

    fn reset_failure_count(&self, state: &mut CircuitBreakerState) {
        state.failure_count = 0;
        state.last_failure_time = None;
    }

    /// Get current circuit state
    pub fn state(&self) -> CircuitState {
        self.state.read().state
    }

    /// Get circuit breaker statistics
    pub fn stats(&self) -> CircuitBreakerStats {
        let state = self.state.read();
        CircuitBreakerStats {
            name: self.name.clone(),
            state: state.state,
            failure_count: state.failure_count,
            success_count: state.success_count,
            total_requests: self.total_requests.load(Ordering::Relaxed),
            total_failures: self.total_failures.load(Ordering::Relaxed),
            total_successes: self.total_successes.load(Ordering::Relaxed),
            total_rejected: self.total_rejected.load(Ordering::Relaxed),
        }
    }

    /// Force reset the circuit breaker
    pub fn reset(&self) {
        let mut state = self.state.write();
        state.state = CircuitState::Closed;
        state.failure_count = 0;
        state.success_count = 0;
        state.last_failure_time = None;
        state.opened_at = None;
        debug!(circuit_breaker = %self.name, "Circuit breaker reset");
    }

    /// Force open the circuit breaker
    pub fn trip(&self) {
        let mut state = self.state.write();
        state.state = CircuitState::Open;
        state.opened_at = Some(Instant::now());
        warn!(circuit_breaker = %self.name, "Circuit breaker manually tripped");
    }
}

/// Statistics for a circuit breaker
#[derive(Debug, Clone)]
pub struct CircuitBreakerStats {
    pub name: String,
    pub state: CircuitState,
    pub failure_count: u64,
    pub success_count: u64,
    pub total_requests: u64,
    pub total_failures: u64,
    pub total_successes: u64,
    pub total_rejected: u64,
}

impl std::fmt::Display for CircuitBreakerStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CircuitBreaker[{}] state={}, requests={}, successes={}, failures={}, rejected={}",
            self.name,
            self.state,
            self.total_requests,
            self.total_successes,
            self.total_failures,
            self.total_rejected
        )
    }
}

/// Execute an operation with circuit breaker protection
pub async fn with_circuit_breaker<F, Fut, T, E>(
    circuit_breaker: &CircuitBreaker,
    operation: F,
) -> Result<T, CircuitBreakerError<E>>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    if !circuit_breaker.is_allowed() {
        return Err(CircuitBreakerError::CircuitOpen);
    }

    match operation().await {
        Ok(result) => {
            circuit_breaker.record_success();
            Ok(result)
        }
        Err(e) => {
            circuit_breaker.record_failure();
            Err(CircuitBreakerError::OperationFailed(e))
        }
    }
}

/// Error type for circuit breaker operations
#[derive(Debug)]
pub enum CircuitBreakerError<E> {
    /// Circuit is open, request was rejected
    CircuitOpen,
    /// Operation failed
    OperationFailed(E),
}

impl<E: std::fmt::Display> std::fmt::Display for CircuitBreakerError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitBreakerError::CircuitOpen => write!(f, "Circuit breaker is open"),
            CircuitBreakerError::OperationFailed(e) => write!(f, "Operation failed: {}", e),
        }
    }
}

impl<E: std::fmt::Debug + std::fmt::Display> std::error::Error for CircuitBreakerError<E> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_default_config() {
        let config = CircuitBreakerConfig::default();
        assert_eq!(config.failure_threshold, 5);
        assert_eq!(config.recovery_timeout, Duration::from_secs(30));
        assert_eq!(config.success_threshold, 3);
        assert_eq!(config.failure_window, Duration::from_secs(60));
    }

    #[test]
    fn test_circuit_breaker_closed_state() {
        let cb = CircuitBreaker::new("test", CircuitBreakerConfig::default());
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.is_allowed());
    }

    #[test]
    fn test_circuit_breaker_opens_after_failures() {
        let config = CircuitBreakerConfig::default().with_failure_threshold(3);
        let cb = CircuitBreaker::new("test", config);

        assert!(cb.is_allowed());
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        assert!(cb.is_allowed());
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        assert!(cb.is_allowed());
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        assert!(!cb.is_allowed());
    }

    #[test]
    fn test_circuit_breaker_resets_on_success() {
        let config = CircuitBreakerConfig::default()
            .with_failure_threshold(2)
            .with_success_threshold(2);
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
    }
}
