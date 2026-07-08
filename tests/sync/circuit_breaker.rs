//! Circuit Breaker Integration Tests (TC-250 ~ TC-255)
//!
//! Tests for circuit breaker integration with external clients

use graphdb::sync::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState};
use std::sync::Arc;
use std::time::Duration;

/// TC-250: Circuit breaker state transitions
#[test]
fn test_circuit_breaker_state_transitions() {
    let config = CircuitBreakerConfig::default();
    let breaker = Arc::new(CircuitBreaker::new("test", config));

    // Initial state should be Closed
    assert_eq!(breaker.state(), CircuitState::Closed);

    // Simulate failures to open circuit (threshold is 5 by default)
    for _ in 0..5 {
        breaker.record_failure();
    }

    // Circuit should be Open
    assert_eq!(breaker.state(), CircuitState::Open);

    // Manually set to half-open for testing
    breaker.reset();
    breaker.record_failure();
    breaker.record_failure();
    breaker.record_failure();
    breaker.record_failure();
    breaker.record_failure();
    assert_eq!(breaker.state(), CircuitState::Open);

    breaker.reset();
    assert_eq!(breaker.state(), CircuitState::Closed);
}

/// TC-251: Circuit breaker blocks requests when open
#[test]
fn test_circuit_breaker_blocks_when_open() {
    let config = CircuitBreakerConfig::default();
    let breaker = Arc::new(CircuitBreaker::new("test", config));

    // Open the circuit
    for _ in 0..5 {
        breaker.record_failure();
    }

    assert_eq!(breaker.state(), CircuitState::Open);

    // Check if requests are blocked
    assert!(
        !breaker.is_allowed(),
        "Circuit should block requests when open"
    );
}

/// TC-252: Circuit breaker half-open state
#[test]
fn test_circuit_breaker_half_open() {
    let config = CircuitBreakerConfig::default();
    let breaker = Arc::new(CircuitBreaker::new("test", config));

    // Open the circuit
    for _ in 0..5 {
        breaker.record_failure();
    }

    assert_eq!(breaker.state(), CircuitState::Open);

    // Reset to test half-open behavior
    breaker.reset();
    assert_eq!(breaker.state(), CircuitState::Closed);

    // Verify requests are allowed again
    assert!(
        breaker.is_allowed(),
        "Circuit should allow requests when closed"
    );
}

/// TC-253: Circuit breaker with concurrent access
#[test]
fn test_circuit_breaker_concurrent_access() {
    let config = CircuitBreakerConfig::default();
    let breaker = Arc::new(CircuitBreaker::new("test", config));

    let mut handles = vec![];

    // Spawn multiple tasks that record failures
    for _ in 0..10 {
        let breaker = breaker.clone();
        let handle = std::thread::spawn(move || {
            breaker.record_failure();
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    // Circuit should be open after threshold failures
    assert_eq!(breaker.state(), CircuitState::Open);
}

/// TC-254: Circuit breaker success counting
#[test]
fn test_circuit_breaker_success_counting() {
    let config = CircuitBreakerConfig::default();
    let breaker = Arc::new(CircuitBreaker::new("test", config));

    // Record some successes
    for _ in 0..3 {
        breaker.record_success();
    }

    let stats = breaker.stats();
    assert_eq!(stats.total_successes, 3);
    assert_eq!(stats.total_failures, 0);

    // Circuit should remain closed
    assert_eq!(breaker.state(), CircuitState::Closed);
}

/// TC-255: Circuit breaker reset
#[test]
fn test_circuit_breaker_reset() {
    let config = CircuitBreakerConfig::default();
    let breaker = Arc::new(CircuitBreaker::new("test", config));

    // Open the circuit
    for _ in 0..5 {
        breaker.record_failure();
    }

    assert_eq!(breaker.state(), CircuitState::Open);

    // Reset the circuit breaker
    breaker.reset();

    // Should be closed immediately
    assert_eq!(breaker.state(), CircuitState::Closed);

    // Should allow operations
    assert!(
        breaker.is_allowed(),
        "Circuit should allow requests after reset"
    );
}

/// TC-256: Circuit breaker with custom config
#[test]
fn test_circuit_breaker_custom_config() {
    let config = CircuitBreakerConfig::new(
        3,                       // failure_threshold
        Duration::from_secs(10), // recovery_timeout
        2,                       // success_threshold
        Duration::from_secs(30), // failure_window
    );

    let breaker = Arc::new(CircuitBreaker::new("custom_test", config));

    // Initial state should be Closed
    assert_eq!(breaker.state(), CircuitState::Closed);

    // Record failures to open circuit (threshold is 3)
    for _ in 0..3 {
        breaker.record_failure();
    }

    assert_eq!(breaker.state(), CircuitState::Open);

    // Verify circuit is blocking
    assert!(!breaker.is_allowed());
}

/// TC-257: Circuit breaker statistics
#[test]
fn test_circuit_breaker_statistics() {
    let config = CircuitBreakerConfig::default();
    let breaker = Arc::new(CircuitBreaker::new("stats_test", config));

    // Record some operations
    for _ in 0..5 {
        breaker.record_success();
    }
    for _ in 0..3 {
        breaker.record_failure();
    }

    let stats = breaker.stats();
    assert_eq!(stats.total_successes, 5);
    assert_eq!(stats.total_failures, 3);
    assert_eq!(stats.name, "stats_test");
}

/// TC-258: Circuit breaker trip manually
#[test]
fn test_circuit_breaker_trip() {
    let config = CircuitBreakerConfig::default();
    let breaker = Arc::new(CircuitBreaker::new("trip_test", config));

    // Circuit should be closed initially
    assert_eq!(breaker.state(), CircuitState::Closed);

    // Trip the circuit manually
    breaker.trip();

    // Circuit should be open
    assert_eq!(breaker.state(), CircuitState::Open);

    // Should block requests
    assert!(!breaker.is_allowed());
}

/// TC-259: Circuit breaker with async operation
#[tokio::test]
async fn test_circuit_breaker_async_operation() {
    use graphdb::sync::circuit_breaker::with_circuit_breaker;

    let config = CircuitBreakerConfig::default();
    let breaker = Arc::new(CircuitBreaker::new("async_test", config));

    // Successful operation
    let result = with_circuit_breaker(&breaker, || async { Ok::<(), String>(()) }).await;

    assert!(result.is_ok());
    assert_eq!(breaker.state(), CircuitState::Closed);

    // Open the circuit
    for _ in 0..5 {
        breaker.record_failure();
    }

    // Operation should be rejected
    let result = with_circuit_breaker(&breaker, || async { Ok::<(), String>(()) }).await;

    assert!(result.is_err());
}

/// TC-261: Circuit breaker half-open auto transition via recovery timeout
///
/// Verifies that the circuit breaker automatically transitions from
/// Open to Half-Open when the recovery_timeout elapses, even without
/// manual intervention.
#[test]
fn test_circuit_breaker_half_open_auto_transition() {
    let config = CircuitBreakerConfig::new(
        3,                         // failure_threshold
        Duration::from_millis(50), // recovery_timeout (short for testing)
        2,                         // success_threshold
        Duration::from_secs(60),   // failure_window
    );

    let breaker = Arc::new(CircuitBreaker::new("auto_transition", config));

    // Open the circuit
    for _ in 0..3 {
        breaker.record_failure();
    }
    assert_eq!(breaker.state(), CircuitState::Open);
    assert!(!breaker.is_allowed(), "Should block when Open");

    // Wait for recovery timeout to pass
    std::thread::sleep(Duration::from_millis(100));

    // After recovery_timeout, is_allowed() should trigger Half-Open transition
    assert!(
        breaker.is_allowed(),
        "Should allow after recovery_timeout (Half-Open)"
    );
    assert_eq!(
        breaker.state(),
        CircuitState::HalfOpen,
        "Should auto-transition to Half-Open"
    );
}

/// TC-262: Circuit breaker half-open → closed via successful operations
///
/// Verifies that after recovering to Half-Open, successive successful
/// operations transition the breaker back to Closed.
#[test]
fn test_circuit_breaker_half_open_to_closed() {
    let config = CircuitBreakerConfig::new(
        3,                         // failure_threshold
        Duration::from_millis(50), // recovery_timeout
        2,                         // success_threshold (need 2 successes)
        Duration::from_secs(60),   // failure_window
    );

    let breaker = Arc::new(CircuitBreaker::new("half_to_closed", config));

    // Open the circuit
    for _ in 0..3 {
        breaker.record_failure();
    }
    assert_eq!(breaker.state(), CircuitState::Open);

    // Wait for recovery timeout
    std::thread::sleep(Duration::from_millis(100));

    // Trigger Half-Open
    assert!(breaker.is_allowed());
    assert_eq!(breaker.state(), CircuitState::HalfOpen);

    // Record success_count threshold successes to close
    breaker.record_success();
    assert_eq!(
        breaker.state(),
        CircuitState::HalfOpen,
        "Still Half-Open after 1 success (needs 2)"
    );

    breaker.record_success();
    assert_eq!(
        breaker.state(),
        CircuitState::Closed,
        "Should transition to Closed after reaching success threshold"
    );
}

/// TC-263: Circuit breaker half-open → open on failure
///
/// Verifies that a failure in Half-Open state reopens the circuit immediately.
#[test]
fn test_circuit_breaker_half_open_reopens_on_failure() {
    let config = CircuitBreakerConfig::new(
        3,                         // failure_threshold
        Duration::from_millis(50), // recovery_timeout
        2,                         // success_threshold
        Duration::from_secs(60),   // failure_window
    );

    let breaker = Arc::new(CircuitBreaker::new("reopen_on_fail", config));

    // Open the circuit
    for _ in 0..3 {
        breaker.record_failure();
    }

    // Wait for recovery timeout
    std::thread::sleep(Duration::from_millis(100));

    // Trigger Half-Open
    assert!(breaker.is_allowed());
    assert_eq!(breaker.state(), CircuitState::HalfOpen);

    // A failure in Half-Open should reopen
    breaker.record_failure();
    assert_eq!(
        breaker.state(),
        CircuitState::Open,
        "Failure in Half-Open should reopen circuit"
    );
    assert!(!breaker.is_allowed(), "Should block after reopen");
}
