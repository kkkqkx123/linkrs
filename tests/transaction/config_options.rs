//! Transaction Configuration Options Tests
//!
//! Test coverage for transaction manager and transaction options:
//! - TransactionManagerConfig default and custom values
//! - TransactionOptions builder pattern
//! - TransactionConfig builder pattern
//! - RetryConfig builder pattern
//! - DurabilityLevel conversion
//! - IsolationLevel settings
//! - Timeout configurations
//! - Auto-cleanup functionality

use graphdb::transaction::{
    DurabilityLevel, IsolationLevel, RetryConfig, TransactionConfig, TransactionManager,
    TransactionManagerConfig, TransactionOptions,
};
use std::time::Duration;

/// Test TransactionManagerConfig default values
#[test]
fn test_manager_config_default() {
    let config = TransactionManagerConfig::default();

    assert_eq!(config.default_timeout, Duration::from_secs(30));
    assert_eq!(config.max_concurrent_transactions, 1000);
    assert!(config.auto_cleanup);
}

/// Test TransactionManagerConfig custom values
#[test]
fn test_manager_config_custom() {
    let config = TransactionManagerConfig {
        default_timeout: Duration::from_secs(60),
        max_concurrent_transactions: 500,
        auto_cleanup: false,
        write_lock_timeout: Duration::from_secs(10),
    };

    assert_eq!(config.default_timeout, Duration::from_secs(60));
    assert_eq!(config.max_concurrent_transactions, 500);
    assert!(!config.auto_cleanup);
}

/// Test TransactionOptions default values
#[test]
fn test_transaction_options_default() {
    let options = TransactionOptions::default();

    assert_eq!(options.timeout, None);
    assert!(!options.read_only);
    assert_eq!(options.durability, DurabilityLevel::Sync);
    assert_eq!(options.isolation_level, IsolationLevel::RepeatableRead);
    assert_eq!(options.query_timeout, None);
    assert_eq!(options.statement_timeout, None);
    assert_eq!(options.idle_timeout, None);
    assert!(!options.two_phase_commit);
}

/// Test TransactionOptions builder pattern
#[test]
fn test_transaction_options_builder() {
    let options = TransactionOptions::new()
        .with_timeout(Duration::from_secs(120))
        .read_only()
        .with_durability(DurabilityLevel::None)
        .with_isolation_level(IsolationLevel::RepeatableRead)
        .with_query_timeout(Duration::from_secs(10))
        .with_statement_timeout(Duration::from_secs(5))
        .with_idle_timeout(Duration::from_secs(300));

    assert_eq!(options.timeout, Some(Duration::from_secs(120)));
    assert!(options.read_only);
    assert_eq!(options.durability, DurabilityLevel::None);
    assert_eq!(options.isolation_level, IsolationLevel::RepeatableRead);
    assert_eq!(options.query_timeout, Some(Duration::from_secs(10)));
    assert_eq!(options.statement_timeout, Some(Duration::from_secs(5)));
    assert_eq!(options.idle_timeout, Some(Duration::from_secs(300)));
}

/// Test TransactionOptions with two-phase commit
#[test]
fn test_transaction_options_two_phase_commit() {
    let mut options = TransactionOptions::new();
    options.two_phase_commit = true;

    assert!(options.two_phase_commit);
}

/// Test TransactionConfig default values
#[test]
fn test_transaction_config_default() {
    let config = TransactionConfig::default();

    assert_eq!(config.timeout, Duration::from_secs(30));
    assert_eq!(config.durability, DurabilityLevel::Sync);
    assert_eq!(config.isolation_level, IsolationLevel::RepeatableRead);
    assert_eq!(config.query_timeout, None);
    assert_eq!(config.statement_timeout, None);
    assert_eq!(config.idle_timeout, None);
    assert!(!config.two_phase_commit);
}

/// Test TransactionConfig builder pattern
#[test]
fn test_transaction_config_builder() {
    let config = TransactionConfig::new()
        .with_timeout(Duration::from_secs(60))
        .with_durability(DurabilityLevel::None)
        .with_isolation_level(IsolationLevel::RepeatableRead)
        .with_query_timeout(Some(Duration::from_secs(15)))
        .with_statement_timeout(Some(Duration::from_secs(7)))
        .with_idle_timeout(Some(Duration::from_secs(600)))
        .with_two_phase_commit(true);

    assert_eq!(config.timeout, Duration::from_secs(60));
    assert_eq!(config.durability, DurabilityLevel::None);
    assert_eq!(config.isolation_level, IsolationLevel::RepeatableRead);
    assert_eq!(config.query_timeout, Some(Duration::from_secs(15)));
    assert_eq!(config.statement_timeout, Some(Duration::from_secs(7)));
    assert_eq!(config.idle_timeout, Some(Duration::from_secs(600)));
    assert!(config.two_phase_commit);
}

/// Test RetryConfig default values
#[test]
fn test_retry_config_default() {
    let config = RetryConfig::default();

    assert_eq!(config.max_retries, 3);
    assert_eq!(config.initial_delay, Duration::from_millis(100));
    assert_eq!(config.backoff_multiplier, 2.0);
    assert_eq!(config.max_delay, Duration::from_secs(10));
}

/// Test RetryConfig builder pattern
#[test]
fn test_retry_config_builder() {
    let config = RetryConfig::new()
        .with_max_retries(5)
        .with_initial_delay(Duration::from_millis(200))
        .with_backoff_multiplier(3.0)
        .with_max_delay(Duration::from_secs(30));

    assert_eq!(config.max_retries, 5);
    assert_eq!(config.initial_delay, Duration::from_millis(200));
    assert_eq!(config.backoff_multiplier, 3.0);
    assert_eq!(config.max_delay, Duration::from_secs(30));
}

/// Test DurabilityLevel equality
#[test]
fn test_durability_level_equality() {
    assert_eq!(DurabilityLevel::None, DurabilityLevel::None);
    assert_eq!(DurabilityLevel::Sync, DurabilityLevel::Sync);
    assert_ne!(DurabilityLevel::None, DurabilityLevel::Sync);
}

/// Test IsolationLevel default and display
#[test]
fn test_isolation_level() {
    let default = IsolationLevel::default();
    assert_eq!(default, IsolationLevel::RepeatableRead);

    assert_eq!(
        format!("{}", IsolationLevel::RepeatableRead),
        "REPEATABLE READ"
    );
}

/// Test TransactionManager with custom config
#[test]
fn test_manager_with_custom_config() {
    let config = TransactionManagerConfig {
        default_timeout: Duration::from_secs(60),
        max_concurrent_transactions: 100,
        auto_cleanup: true,
        write_lock_timeout: Duration::from_secs(10),
    };

    let manager = TransactionManager::new(config);

    // Verify config is stored correctly
    let stored_config = manager.config();
    assert_eq!(stored_config.default_timeout, Duration::from_secs(60));
    assert_eq!(stored_config.max_concurrent_transactions, 100);
    assert!(stored_config.auto_cleanup);

    // Begin transaction should use default timeout
    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

/// Test transaction with various timeout combinations
#[test]
fn test_transaction_timeout_combinations() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    // Test with only transaction timeout
    let options1 = TransactionOptions::new().with_timeout(Duration::from_secs(30));
    let txn1 = manager
        .begin_transaction(options1)
        .expect("Failed to begin transaction 1");

    let context1 = manager.get_context(txn1).expect("Failed to get context");
    assert!(!context1.is_expired());
    manager
        .commit_transaction(txn1)
        .expect("Failed to commit transaction 1");

    // Test with all timeouts set
    let options2 = TransactionOptions::new()
        .with_timeout(Duration::from_secs(60))
        .with_query_timeout(Duration::from_secs(10))
        .with_statement_timeout(Duration::from_secs(5))
        .with_idle_timeout(Duration::from_secs(300));

    let txn2 = manager
        .begin_transaction(options2)
        .expect("Failed to begin transaction 2");

    let context2 = manager.get_context(txn2).expect("Failed to get context");
    assert!(context2.query_timeout.is_some());
    assert!(context2.statement_timeout.is_some());
    assert!(context2.idle_timeout.is_some());

    manager
        .commit_transaction(txn2)
        .expect("Failed to commit transaction 2");
}

/// Test read-only transaction options
#[test]
fn test_readonly_transaction_options() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    // Test read-only with various options
    let options = TransactionOptions::new()
        .read_only()
        .with_timeout(Duration::from_secs(30))
        .with_query_timeout(Duration::from_secs(10));

    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin read-only transaction");

    let context = manager.get_context(txn_id).expect("Failed to get context");
    assert!(context.read_only);
    assert_eq!(context.durability, DurabilityLevel::Sync); // Read-only always uses Sync

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit read-only transaction");
}

/// Test high-performance write options
#[test]
fn test_high_performance_write_options() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    // Test with None durability for high performance
    let options = TransactionOptions::new().with_durability(DurabilityLevel::None);

    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    let context = manager.get_context(txn_id).expect("Failed to get context");
    assert_eq!(context.durability, DurabilityLevel::None);

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

/// Test repeatable read isolation options
#[test]
fn test_repeatable_read_options() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let options = TransactionOptions::new().with_isolation_level(IsolationLevel::RepeatableRead);

    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    let context = manager.get_context(txn_id).expect("Failed to get context");
    assert_eq!(context.isolation_level, IsolationLevel::RepeatableRead);

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

/// Test retry config with exponential backoff calculation
#[test]
fn test_retry_config_backoff_calculation() {
    let config = RetryConfig::new()
        .with_initial_delay(Duration::from_millis(100))
        .with_backoff_multiplier(2.0)
        .with_max_delay(Duration::from_secs(5));

    // Calculate expected delays
    let delay1 = config.initial_delay;
    let delay2 = Duration::from_secs_f64(delay1.as_secs_f64() * config.backoff_multiplier);
    let delay3 = Duration::from_secs_f64(delay2.as_secs_f64() * config.backoff_multiplier);

    assert_eq!(delay1, Duration::from_millis(100));
    assert_eq!(delay2, Duration::from_millis(200));
    assert_eq!(delay3, Duration::from_millis(400));
}

/// Test TransactionOptions clone
#[test]
fn test_transaction_options_clone() {
    let original = TransactionOptions::new()
        .with_timeout(Duration::from_secs(30))
        .read_only()
        .with_durability(DurabilityLevel::None);

    let cloned = original.clone();

    assert_eq!(original.timeout, cloned.timeout);
    assert_eq!(original.read_only, cloned.read_only);
    assert_eq!(original.durability, cloned.durability);
}

/// Test TransactionConfig clone
#[test]
fn test_transaction_config_clone() {
    let original = TransactionConfig::new()
        .with_timeout(Duration::from_secs(30))
        .with_durability(DurabilityLevel::None)
        .with_two_phase_commit(true);

    let cloned = original.clone();

    assert_eq!(original.timeout, cloned.timeout);
    assert_eq!(original.durability, cloned.durability);
    assert_eq!(original.two_phase_commit, cloned.two_phase_commit);
}

/// Test RetryConfig clone and copy
#[test]
fn test_retry_config_clone_copy() {
    let original = RetryConfig::new()
        .with_max_retries(5)
        .with_initial_delay(Duration::from_millis(200));

    let copied = original;

    assert_eq!(original.max_retries, copied.max_retries);
}

/// Test TransactionManagerConfig clone
#[test]
fn test_manager_config_clone() {
    let original = TransactionManagerConfig {
        default_timeout: Duration::from_secs(45),
        max_concurrent_transactions: 200,
        auto_cleanup: false,
        write_lock_timeout: Duration::from_secs(10),
    };

    let cloned = original.clone();

    assert_eq!(original.default_timeout, cloned.default_timeout);
    assert_eq!(
        original.max_concurrent_transactions,
        cloned.max_concurrent_transactions
    );
    assert_eq!(original.auto_cleanup, cloned.auto_cleanup);
}

/// Test edge case: zero timeout
#[tokio::test]
async fn test_zero_timeout() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    // Zero timeout should immediately expire
    let options = TransactionOptions::new().with_timeout(Duration::from_secs(0));
    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    // Small delay to ensure expiration
    tokio::time::sleep(Duration::from_millis(10)).await;

    let context = manager.get_context(txn_id).expect("Failed to get context");
    assert!(context.is_expired());

    // Commit should fail with timeout
    let result = manager.commit_transaction(txn_id);
    assert!(result.is_err());
}

/// Test edge case: very long timeout
#[test]
fn test_very_long_timeout() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    // Very long timeout (1 hour)
    let options = TransactionOptions::new().with_timeout(Duration::from_secs(3600));
    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    let context = manager.get_context(txn_id).expect("Failed to get context");
    assert!(!context.is_expired());

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}
