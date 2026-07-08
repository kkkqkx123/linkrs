//! Transaction management module
//!
//! Provides full transaction management functionality, including savepoint support

use crate::api::core::{CoreError, CoreResult, QueryRequest, TransactionHandle};
use crate::api::embedded::result::QueryResult;
use crate::api::embedded::session::Session;
use crate::core::Value;
use crate::storage::StorageClient;
use crate::transaction::types::{SavepointId, SavepointInfo};
use crate::transaction::{DurabilityLevel, IsolationLevel, TransactionOptions};
use std::collections::HashMap;
use std::time::Duration;

/// Transaction configuration options
///
/// Used to configure the behavior of transactions, such as timeout, read-only mode, persistence level, etc.
///
/// # Example
///
/// ```rust
/// use graphdb::api::embedded::{GraphDatabase, DatabaseConfig, TransactionConfig};
///
/// # fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let db = GraphDatabase::open("my_db")?;
/// let session = db.session()?;
///
// Create a read-only transaction configuration
/// let config = TransactionConfig::new()
///     .read_only()
///     .with_timeout(Duration::from_secs(60));
///
/// let txn = session.begin_transaction_with_config(config)?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct TransactionConfig {
    /// Transaction timeout
    pub timeout: Option<Duration>,
    /// Read-only or not
    pub read_only: bool,
    /// Persistence level
    pub durability: DurabilityLevel,
    /// Isolation level
    pub isolation_level: IsolationLevel,
    /// Query timeout
    pub query_timeout: Option<Duration>,
    /// Statement timeout
    pub statement_timeout: Option<Duration>,
    /// Idle timeout
    pub idle_timeout: Option<Duration>,
}

impl Default for TransactionConfig {
    fn default() -> Self {
        Self {
            timeout: None,
            read_only: false,
            durability: DurabilityLevel::Sync,
            isolation_level: IsolationLevel::default(),
            query_timeout: None,
            statement_timeout: None,
            idle_timeout: None,
        }
    }
}

impl TransactionConfig {
    /// Creating a Default Configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Setting the timeout period
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set to read-only mode
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    /// Setting Persistence Levels
    pub fn with_durability(mut self, durability: DurabilityLevel) -> Self {
        self.durability = durability;
        self
    }

    /// Set isolation level
    pub fn with_isolation_level(mut self, level: IsolationLevel) -> Self {
        self.isolation_level = level;
        self
    }

    /// Set query timeout
    pub fn with_query_timeout(mut self, timeout: Duration) -> Self {
        self.query_timeout = Some(timeout);
        self
    }

    /// Set statement timeout
    pub fn with_statement_timeout(mut self, timeout: Duration) -> Self {
        self.statement_timeout = Some(timeout);
        self
    }

    /// Set idle timeout
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = Some(timeout);
        self
    }

    /// Convert to internal TransactionOptions
    pub(crate) fn into_options(self) -> TransactionOptions {
        TransactionOptions {
            timeout: self.timeout,
            read_only: self.read_only,
            durability: self.durability,
            isolation_level: self.isolation_level,
            query_timeout: self.query_timeout,
            statement_timeout: self.statement_timeout,
            idle_timeout: self.idle_timeout,
            two_phase_commit: false,
        }
    }
}

/// transaction handle
///
/// Encapsulate transaction lifecycle management to ensure that transactions are properly committed or rolled back
/// Supports savepoint functionality to allow partial rollback
///
/// # Examples
///
/// ```rust
/// use graphdb::api::embedded::{GraphDatabase, DatabaseConfig};
///
/// # fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let db = GraphDatabase::open("my_db")?;
/// let session = db.session()?;
///
// Starting a transaction
/// let txn = session.begin_transaction()?;
///
// Execute queries in transactions
/// txn.execute("CREATE TAG user(name string)")?;
/// txn.execute("INSERT VERTEX user(name) VALUES \"1\":(\"Alice\")")?;
///
// Commit the transaction
/// txn.commit()?;
/// # Ok(())
/// # }
/// ```
pub struct Transaction<'sess, S: StorageClient + Clone + 'static> {
    session: &'sess Session<S>,
    txn_handle: TransactionHandle,
    committed: bool,
    rolled_back: bool,
}

impl<'sess, S: StorageClient + Clone + 'static + graphdb_storage::storage::UndoTarget>
    Transaction<'sess, S>
{
    /// Creating a new transaction
    pub(crate) fn new(session: &'sess Session<S>, txn_handle: TransactionHandle) -> Self {
        Self {
            session,
            txn_handle,
            committed: false,
            rolled_back: false,
        }
    }

    /// Executing queries in a transaction
    ///
    /// # Parameters
    /// - `query` - query statement string
    ///
    /// # Return
    /// - Returns query results on success
    /// - Return an error when something goes wrong.
    ///
    /// # Error
    /// - Returns an error if the transaction has been committed or rolled back
    pub fn execute(&self, query: &str) -> CoreResult<QueryResult> {
        self.check_active()?;

        let txn_manager = self.session.txn_manager();
        let ctx = txn_manager.get_context(self.txn_handle.0)?;
        ctx.check_timeouts().map_err(|e| {
            CoreError::TransactionFailed(format!("Transaction timeout: {}", e))
        })?;

        let query_ctx = QueryRequest {
            space_id: self.session.space_id(),
            space_name: self.session.space_name().map(|s| s.to_string()),
            auto_commit: false,
            transaction_id: Some(self.txn_handle.0),
            parameters: None,
        };

        let mut query_api = self.session.query_api_mut();
        let result = query_api.execute(query, query_ctx)?;
        ctx.update_activity();
        Ok(QueryResult::from_core(result))
    }

    /// Executing parameterized queries in a transaction
    ///
    /// # Parameters
    /// - `query` - query statement string
    /// - `params` - query parameters
    ///
    /// # Back
    /// - Returns query results on success
    /// - Return error on failure
    pub fn execute_with_params(
        &self,
        query: &str,
        params: HashMap<String, Value>,
    ) -> CoreResult<QueryResult> {
        self.check_active()?;

        let txn_manager = self.session.txn_manager();
        let ctx = txn_manager.get_context(self.txn_handle.0)?;
        ctx.check_timeouts().map_err(|e| {
            CoreError::TransactionFailed(format!("Transaction timeout: {}", e))
        })?;

        let query_ctx = QueryRequest {
            space_id: self.session.space_id(),
            space_name: self.session.space_name().map(|s| s.to_string()),
            auto_commit: false,
            transaction_id: Some(self.txn_handle.0),
            parameters: Some(params),
        };

        let mut query_api = self.session.query_api_mut();
        let result = query_api.execute(query, query_ctx)?;
        ctx.update_activity();
        Ok(QueryResult::from_core(result))
    }

    /// Submission of transactions
    ///
    /// # Return
    /// Return () on success.
    /// - Return error on failure
    ///
    /// # Note
    /// Cannot be reused after a transaction has been committed
    pub fn commit(mut self) -> CoreResult<()> {
        self.check_active()?;

        let txn_manager = self.session.txn_manager();
        let ctx = txn_manager.get_context(self.txn_handle.0)?;
        ctx.check_timeouts().map_err(|e| {
            CoreError::TransactionFailed(format!("Transaction timeout: {}", e))
        })?;

        txn_manager
            .commit_transaction(self.txn_handle.0)
            .map_err(|e| crate::api::core::CoreError::TransactionFailed(e.to_string()))?;
        self.committed = true;
        Ok(())
    }

    /// Rolling back transactions
    ///
    /// # Return
    /// - Returns on success ()
    /// - Return error on failure
    ///
    /// # Attention.
    /// Transaction rollback cannot be reused after
    pub fn rollback(mut self) -> CoreResult<()> {
        self.check_active()?;

        let txn_manager = self.session.txn_manager();
        let ctx = txn_manager.get_context(self.txn_handle.0)?;
        ctx.check_timeouts().map_err(|e| {
            CoreError::TransactionFailed(format!("Transaction timeout: {}", e))
        })?;

        txn_manager
            .abort_transaction(self.txn_handle.0)
            .map_err(|e| crate::api::core::CoreError::TransactionFailed(e.to_string()))?;

        self.rolled_back = true;
        Ok(())
    }

    /// Creating a save point
    ///
    /// A save point allows the creation of a marker within a transaction, enabling rollback to that point without affecting the entire transaction.
    ///
    /// # Parameters
    /// `name` – Name of the save point (optional)
    ///
    /// # Return
    /// Return the save point ID when successful.
    /// - Return error on failure
    ///
    /// # Examples
    ///
    /// ```rust
    /// use graphdb::api::embedded::GraphDatabase;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let db = GraphDatabase::open("my_db")?;
    /// let session = db.session()?;
    /// let txn = session.begin_transaction()?;
    ///
    // Create a save point
    /// let sp = txn.create_savepoint(Some("checkpoint1".to_string()))?;
    ///
    // Perform some operations…
    /// txn.execute("INSERT VERTEX user(name) VALUES \"1\":(\"Alice\")")?;
    ///
    // If necessary, it is possible to roll back to a previously saved state.
    /// txn.rollback_to_savepoint(sp)?;
    ///
    // Submission of transactions
    /// txn.commit()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn create_savepoint(&self, name: Option<String>) -> CoreResult<SavepointId> {
        self.check_active()?;

        let txn_manager = self.session.txn_manager();
        txn_manager
            .create_savepoint(self.txn_handle.0, name)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))
    }

    /// Roll back to the saved point.
    ///
    /// Roll back to the specified save point; all operations performed after that save point will be undone.
    /// However, the save point itself remains valid and can still be used.
    ///
    /// # Parameters
    /// `savepoint_id` – ID of the savepoint
    ///
    /// # Return
    /// - Returns on success ()
    /// - Return error on failure
    pub fn rollback_to_savepoint(&self, savepoint_id: SavepointId) -> CoreResult<()> {
        self.check_active()?;

        let txn_manager = self.session.txn_manager();
        let storage = self.session.storage_mut();
        txn_manager
            .rollback_to_savepoint(self.txn_handle.0, savepoint_id, &*storage)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))
    }

    /// Release the save point.
    ///
    /// Once a save point is released, it is no longer possible to revert back to that save point. However, no changes will be undone either.
    ///
    /// # Parameters
    /// - `savepoint_id` - savepoint ID
    ///
    /// # Return
    /// - Returns on success ()
    /// - Return error on failure
    pub fn release_savepoint(&self, savepoint_id: SavepointId) -> CoreResult<()> {
        self.check_active()?;

        let txn_manager = self.session.txn_manager();
        txn_manager
            .release_savepoint(self.txn_handle.0, savepoint_id)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))
    }

    /// Find a saved point by its name
    ///
    /// # Parameters
    /// "name" – The name of the save point.
    ///
    /// # Return
    /// - Returns Some(SavepointId) when found
    /// - Return `None` when not found.
    pub fn find_savepoint(&self, name: &str) -> Option<SavepointId> {
        if !self.is_active() {
            return None;
        }

        let context = self
            .session
            .txn_manager()
            .get_context(self.txn_handle.0)
            .ok()?;
        context.find_savepoint_by_name(name).map(|info| info.id)
    }

    /// Retrieve all active save points.
    ///
    /// # Return
    /// List of active save point information
    pub fn list_savepoints(&self) -> Vec<SavepointInfo> {
        if !self.is_active() {
            return Vec::new();
        }

        let txn_manager = self.session.txn_manager();
        txn_manager.get_active_savepoints(self.txn_handle.0)
    }

    /// Obtaining transaction information
    ///
    /// # Return
    /// Return transaction information in case of success.
    /// - Return error on failure
    pub fn info(&self) -> CoreResult<TransactionInfo> {
        let txn_manager = self.session.txn_manager();
        txn_manager
            .get_transaction_info(self.txn_handle.0)
            .map(|info| TransactionInfo {
                id: info.id.0,
                state: format!("{:?}", info.state),
                is_read_only: info.is_read_only,
                elapsed_ms: info.elapsed.as_millis() as u64,
                savepoint_count: info.savepoint_count,
            })
            .ok_or_else(|| CoreError::TransactionFailed("Transaction not found".to_string()))
    }

    /// Check whether the transaction is in an active state.
    fn check_active(&self) -> CoreResult<()> {
        if self.committed {
            return Err(CoreError::TransactionFailed(
                "Transaction has been committed, cannot perform operations".to_string(),
            ));
        }
        if self.rolled_back {
            return Err(CoreError::TransactionFailed(
                "Transaction has been rolled back, cannot perform operations".to_string(),
            ));
        }
        Ok(())
    }

    /// Check whether the transaction has been committed.
    pub fn is_committed(&self) -> bool {
        self.committed
    }

    /// Check whether the transaction has been rolled back.
    pub fn is_rolled_back(&self) -> bool {
        self.rolled_back
    }

    /// Check whether the transaction is still in an active state.
    pub fn is_active(&self) -> bool {
        !self.committed && !self.rolled_back
    }

    /// Obtaining the transaction handle
    ///
    /// Return the unique handle for this transaction, which can be used to track the transaction status across different APIs.
    pub fn handle(&self) -> TransactionHandle {
        self.txn_handle
    }

    /// Get Transaction ID
    pub fn id(&self) -> u64 {
        self.txn_handle.id()
    }

    /// Get transaction handle (for internal use by C API)
    pub(crate) fn txn_handle(&self) -> TransactionHandle {
        self.txn_handle
    }
}

impl<'sess, S: StorageClient + Clone + 'static> Drop for Transaction<'sess, S> {
    fn drop(&mut self) {
        // If the transaction is still active, it will be automatically rolled back.
        if self.is_active() {
            let _ = self
                .session
                .txn_manager()
                .abort_transaction(self.txn_handle.0);
        }
    }
}

/// Transaction information
///
/// Provide detailed information and the status of the transaction.
#[derive(Debug, Clone)]
pub struct TransactionInfo {
    /// Transaction ID
    pub id: u64,
    /// Transaction status
    pub state: String,
    /// Read-only or not
    pub is_read_only: bool,
    /// Running time (milliseconds)
    pub elapsed_ms: u64,
    /// Number of save points
    pub savepoint_count: usize,
}

impl TransactionInfo {
    /// Get Transaction ID
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Getting Transaction Status
    pub fn state(&self) -> &str {
        &self.state
    }

    /// Check for read-only
    pub fn is_read_only(&self) -> bool {
        self.is_read_only
    }

    /// Get Running Time (milliseconds)
    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms
    }

    /// Get the number of savepoints
    pub fn savepoint_count(&self) -> usize {
        self.savepoint_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_config_default() {
        let config = TransactionConfig::default();
        assert!(!config.read_only);
        assert!(config.timeout.is_none());
    }

    #[test]
    fn test_transaction_config_builder() {
        let config = TransactionConfig::new()
            .read_only()
            .with_timeout(Duration::from_secs(60))
            .with_durability(DurabilityLevel::None);

        assert!(config.read_only);
        assert_eq!(config.timeout, Some(Duration::from_secs(60)));
        assert_eq!(config.durability, DurabilityLevel::None);
    }

    #[test]
    fn test_transaction_config_into_options() {
        let config = TransactionConfig::new()
            .read_only()
            .with_timeout(Duration::from_secs(30));

        let options = config.into_options();
        assert!(options.read_only);
        assert_eq!(options.timeout, Some(Duration::from_secs(30)));
    }
}
