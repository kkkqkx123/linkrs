//! Transaction Testing Helpers
//!
//! Provides helper functions and utilities for transaction integration tests

use graphdb::core::error::DBResult;
use graphdb::core::Value;
use graphdb::transaction::{
    TransactionError, TransactionId, TransactionManager, TransactionOptions, TransactionState,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Convert TransactionError to DBError
fn txn_err_to_db_err(e: TransactionError) -> graphdb::core::error::DBError {
    graphdb::core::error::DBError::transaction(e.to_string())
}

/// Transaction test context for managing test transactions
pub struct TransactionTestContext {
    manager: Arc<Mutex<TransactionManager>>,
    active_transactions: HashMap<String, TransactionId>,
}

impl TransactionTestContext {
    /// Create a new transaction test context
    pub fn new(manager: Arc<Mutex<TransactionManager>>) -> Self {
        Self {
            manager,
            active_transactions: HashMap::new(),
        }
    }

    /// Begin a new transaction with the given name and options
    pub async fn begin_transaction(
        &mut self,
        name: &str,
        options: TransactionOptions,
    ) -> DBResult<TransactionId> {
        let manager = self.manager.lock().await;
        let txn_id = manager
            .begin_transaction(options)
            .map_err(txn_err_to_db_err)?;
        self.active_transactions.insert(name.to_string(), txn_id);
        Ok(txn_id)
    }

    /// Commit a transaction by name
    pub async fn commit_transaction(&mut self, name: &str) -> DBResult<()> {
        let txn_id = self.active_transactions.remove(name).ok_or_else(|| {
            graphdb::core::error::DBError::transaction(format!("Transaction '{}' not found", name))
        })?;

        let manager = self.manager.lock().await;
        manager
            .commit_transaction(txn_id)
            .map_err(txn_err_to_db_err)?;

        Ok(())
    }

    /// Rollback a transaction by name
    pub async fn rollback_transaction(&mut self, name: &str) -> DBResult<()> {
        let txn_id = self.active_transactions.remove(name).ok_or_else(|| {
            graphdb::core::error::DBError::transaction(format!("Transaction '{}' not found", name))
        })?;

        let manager = self.manager.lock().await;
        manager
            .abort_transaction(txn_id)
            .map_err(txn_err_to_db_err)?;

        Ok(())
    }

    /// Get transaction ID by name
    pub fn get_transaction_id(&self, name: &str) -> Option<TransactionId> {
        self.active_transactions.get(name).copied()
    }

    /// Check if a transaction is active
    pub async fn is_transaction_active(&self, name: &str) -> bool {
        if let Some(&txn_id) = self.active_transactions.get(name) {
            let manager = self.manager.lock().await;
            manager.is_transaction_active(txn_id)
        } else {
            false
        }
    }

    /// Get transaction state
    pub async fn get_transaction_state(&self, name: &str) -> Option<TransactionState> {
        let txn_id = self.active_transactions.get(name)?;
        let manager = self.manager.lock().await;
        manager.get_context(*txn_id).ok().map(|ctx| ctx.state())
    }

    /// Clean up all active transactions
    pub async fn cleanup_all(&mut self) -> DBResult<()> {
        let names: Vec<String> = self.active_transactions.keys().cloned().collect();
        for name in names {
            let _ = self.rollback_transaction(&name).await;
        }
        self.active_transactions.clear();
        Ok(())
    }
}

/// Create default transaction options for write operations
pub fn default_write_options() -> TransactionOptions {
    TransactionOptions::new()
}

/// Create default transaction options for read-only operations
pub fn default_readonly_options() -> TransactionOptions {
    TransactionOptions::new().read_only()
}

/// Create transaction options with timeout
pub fn options_with_timeout(timeout: Duration) -> TransactionOptions {
    TransactionOptions::new().with_timeout(timeout)
}

/// Create transaction options with query timeout
pub fn options_with_query_timeout(timeout: Duration) -> TransactionOptions {
    TransactionOptions::new().with_query_timeout(timeout)
}

/// Create high-performance write options (no immediate durability)
pub fn high_performance_options() -> TransactionOptions {
    use graphdb::transaction::DurabilityLevel;
    TransactionOptions::new().with_durability(DurabilityLevel::None)
}

/// Transaction assertion helpers
pub struct TransactionAssertions;

impl TransactionAssertions {
    /// Assert that a transaction is in the expected state
    pub fn assert_state_eq(actual: TransactionState, expected: TransactionState) {
        assert_eq!(
            actual, expected,
            "Expected transaction state {:?}, got {:?}",
            expected, actual
        );
    }

    /// Assert that a transaction is active
    pub fn assert_active(state: TransactionState) {
        assert!(
            state.can_execute(),
            "Expected transaction to be active, but state is {:?}",
            state
        );
    }

    /// Assert that a transaction has ended
    pub fn assert_ended(state: TransactionState) {
        assert!(
            state.is_terminal(),
            "Expected transaction to be ended, but state is {:?}",
            state
        );
    }
}

/// Concurrent transaction test runner
pub struct ConcurrentTestRunner {
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl ConcurrentTestRunner {
    /// Create a new concurrent test runner
    pub fn new() -> Self {
        Self { tasks: Vec::new() }
    }

    /// Add a task to the runner
    pub fn add_task<F>(&mut self, task: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(task);
        self.tasks.push(handle);
    }

    /// Run all tasks and wait for completion
    pub async fn run_all(self) {
        for task in self.tasks {
            task.await.expect("Task failed");
        }
    }
}

impl Default for ConcurrentTestRunner {
    fn default() -> Self {
        Self::new()
    }
}

/// Test data builders for transaction tests
pub struct TransactionTestData;

impl TransactionTestData {
    /// Create a simple person vertex value map
    pub fn person(name: &str, age: i64) -> HashMap<&'static str, Value> {
        HashMap::from([
            ("name", Value::String(name.into())),
            ("age", Value::Int(age as i32)),
        ])
    }

    /// Create an account vertex value map
    pub fn account(id: i64, balance: i64) -> HashMap<&'static str, Value> {
        HashMap::from([
            ("id", Value::Int(id as i32)),
            ("balance", Value::Int(balance as i32)),
        ])
    }

    /// Create a product vertex value map
    pub fn product(name: &str, price: i64) -> HashMap<&'static str, Value> {
        HashMap::from([
            ("name", Value::String(name.into())),
            ("price", Value::Int(price as i32)),
        ])
    }
}

/// Helper to measure transaction execution time
pub struct TransactionTimer {
    start: std::time::Instant,
}

impl TransactionTimer {
    /// Start a new timer
    pub fn start() -> Self {
        Self {
            start: std::time::Instant::now(),
        }
    }

    /// Get elapsed time
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Assert that execution time is within expected range
    pub fn assert_within(&self, expected: Duration, tolerance: Duration) {
        let elapsed = self.elapsed();
        let diff = elapsed.abs_diff(expected);
        assert!(
            diff <= tolerance,
            "Execution time {:?} differs from expected {:?} by more than {:?}",
            elapsed,
            expected,
            tolerance
        );
    }
}

/// Retry helper for transaction operations
pub async fn with_retry<F, Fut, T>(
    mut operation: F,
    max_retries: u32,
    delay: Duration,
) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let mut last_error = None;

    for attempt in 0..max_retries {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                last_error = Some(e);
                if attempt < max_retries - 1 {
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "Max retries exceeded".to_string()))
}

/// Helper to create batch insert values
pub fn create_batch_values(count: usize, value_fn: impl Fn(usize) -> String) -> String {
    let values: Vec<String> = (0..count).map(value_fn).collect();
    values.join(", ")
}

/// Helper for concurrent transaction stress testing
pub async fn stress_test_transactions(
    manager: Arc<Mutex<TransactionManager>>,
    num_transactions: usize,
    operations_per_transaction: usize,
) -> Result<(), String> {
    let mut handles = vec![];

    for i in 0..num_transactions {
        let manager_clone = Arc::clone(&manager);
        let handle = tokio::spawn(async move {
            let manager = manager_clone.lock().await;
            let options = if i % 2 == 0 {
                default_write_options()
            } else {
                default_readonly_options()
            };

            let txn_id = manager
                .begin_transaction(options)
                .map_err(|e| format!("Failed to begin transaction: {:?}", e))?;

            for _ in 0..operations_per_transaction {
                tokio::task::yield_now().await;
            }

            if i % 3 == 0 {
                manager
                    .abort_transaction(txn_id)
                    .map_err(|e| format!("Failed to rollback: {:?}", e))?;
            } else {
                manager
                    .commit_transaction(txn_id)
                    .map_err(|e| format!("Failed to commit: {:?}", e))?;
            }

            Ok::<(), String>(())
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.map_err(|e| format!("Task failed: {}", e))??;
    }

    Ok(())
}
