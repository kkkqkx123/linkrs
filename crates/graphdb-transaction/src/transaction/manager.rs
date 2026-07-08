//! Transaction Manager
//!
//! Manages the lifecycle of all transactions, providing operations such as
//! transaction start, commit, and abort. Uses MVCC version management for
//! snapshot isolation.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;

use super::cleaner::TransactionCleaner;
use super::context::TransactionContext;
use super::error::TransactionError;
use super::monitor::TransactionMonitor;
use super::rollback::UndoLogRollback;
use super::types::*;
use super::undo_log::UndoTarget;
use super::mvcc::{VersionManager, VersionManagerConfig};
use crate::core::stats::StatsManager;
use crate::sync::SyncManager;

/// Transaction Manager
///
/// Manages the lifecycle of all transactions using MVCC version management.
/// Supports read, insert, update, and compact transactions.
pub struct TransactionManager {
    /// Version manager for MVCC timestamps
    version_manager: Arc<VersionManager>,
    /// Configuration
    config: TransactionManagerConfig,
    /// Active transactions table
    active_transactions: DashMap<TransactionId, Arc<TransactionContext>>,
    /// Transaction ID generator
    id_generator: AtomicU64,
    /// Statistics
    stats: Arc<TransactionStats>,
    /// Whether shutdown
    shutdown_flag: AtomicU64,
    /// Transaction monitor for metrics collection
    monitor: TransactionMonitor,
    /// Transaction cleaner for expired transaction cleanup
    cleaner: TransactionCleaner,
    /// Optional sync manager for index cleanup and commit coordination
    sync_manager: Option<Arc<SyncManager>>,
}

impl TransactionManager {
    /// Create a new transaction manager
    pub fn new(config: TransactionManagerConfig) -> Self {
        let stats = Arc::new(TransactionStats::new());
        let monitor = TransactionMonitor::new(Arc::clone(&stats));
        let version_manager = Arc::new(VersionManager::new());
        let cleaner =
            TransactionCleaner::new(None, Arc::clone(&version_manager), Arc::clone(&stats));

        Self {
            version_manager,
            config,
            active_transactions: DashMap::new(),
            id_generator: AtomicU64::new(1),
            stats,
            shutdown_flag: AtomicU64::new(0),
            monitor,
            cleaner,
            sync_manager: None,
        }
    }

    /// Create a new transaction manager with version manager config
    pub fn with_version_config(
        config: TransactionManagerConfig,
        vm_config: VersionManagerConfig,
    ) -> Self {
        let stats = Arc::new(TransactionStats::new());
        let monitor = TransactionMonitor::new(Arc::clone(&stats));
        let version_manager = Arc::new(VersionManager::with_config(vm_config));
        let cleaner =
            TransactionCleaner::new(None, Arc::clone(&version_manager), Arc::clone(&stats));

        Self {
            version_manager,
            config,
            active_transactions: DashMap::new(),
            id_generator: AtomicU64::new(1),
            stats,
            shutdown_flag: AtomicU64::new(0),
            monitor,
            cleaner,
            sync_manager: None,
        }
    }

    /// Create a new transaction manager with StatsManager integration
    pub fn with_stats_manager(
        config: TransactionManagerConfig,
        stats_manager: Arc<StatsManager>,
    ) -> Self {
        let stats = Arc::new(TransactionStats::with_stats_manager(stats_manager));
        let monitor = TransactionMonitor::new(Arc::clone(&stats));
        let version_manager = Arc::new(VersionManager::new());
        let cleaner =
            TransactionCleaner::new(None, Arc::clone(&version_manager), Arc::clone(&stats));

        Self {
            version_manager,
            config,
            active_transactions: DashMap::new(),
            id_generator: AtomicU64::new(1),
            stats,
            shutdown_flag: AtomicU64::new(0),
            monitor,
            cleaner,
            sync_manager: None,
        }
    }

    /// Attach a sync manager after construction.
    pub fn set_sync_manager(&mut self, sync_manager: Arc<SyncManager>) {
        self.cleaner = TransactionCleaner::new(
            Some(sync_manager.clone()),
            Arc::clone(&self.version_manager),
            Arc::clone(&self.stats),
        );
        self.sync_manager = Some(sync_manager);
    }

    /// Attach a sync manager so transaction completion can clean up index buffers.
    pub fn with_sync_manager(mut self, sync_manager: Arc<SyncManager>) -> Self {
        self.set_sync_manager(sync_manager);
        self
    }

    /// Get the version manager
    pub fn version_manager(&self) -> &Arc<VersionManager> {
        &self.version_manager
    }

    /// Start a new read transaction
    pub fn begin_read_transaction(
        &self,
        options: TransactionOptions,
    ) -> Result<TransactionId, TransactionError> {
        if self.shutdown_flag.load(Ordering::SeqCst) != 0 {
            return Err(TransactionError::internal(
                "Transaction manager is shutdown".to_string(),
            ));
        }

        self.cleanup_expired_transactions();

        let active_count = self.active_transactions.len();
        if active_count >= self.config.max_concurrent_transactions {
            return Err(TransactionError::too_many_transactions());
        }

        let txn_id = TransactionId(self.id_generator.fetch_add(1, Ordering::SeqCst));
        let timestamp = self.version_manager.acquire_read_timestamp();
        let timeout = options.timeout.unwrap_or(self.config.default_timeout);

        let config = TransactionConfig {
            timeout,
            durability: options.durability,
            isolation_level: options.isolation_level,
            query_timeout: options.query_timeout,
            statement_timeout: options.statement_timeout,
            idle_timeout: options.idle_timeout,
            two_phase_commit: options.two_phase_commit,
        };

        let context = Arc::new(TransactionContext::new_readonly(txn_id, timestamp, config));

        self.active_transactions.insert(txn_id, context);
        self.stats.record_txn_begin();

        Ok(txn_id)
    }

    /// Start a snapshot read transaction at a specific timestamp.
    ///
    /// This creates a read-only transaction that sees a consistent snapshot
    /// of the database as of the given timestamp. Useful for:
    /// - Time-travel queries (historical data)
    /// - Consistent backup operations
    /// - Cross-node replication
    ///
    /// # Arguments
    /// - `snapshot_ts`: The timestamp to read from (must be <= current write timestamp)
    /// - `options`: Transaction options (timeout, etc.)
    pub fn begin_snapshot_read(
        &self,
        snapshot_ts: u32,
        options: TransactionOptions,
    ) -> Result<TransactionId, TransactionError> {
        if self.shutdown_flag.load(Ordering::SeqCst) != 0 {
            return Err(TransactionError::internal(
                "Transaction manager is shutdown".to_string(),
            ));
        }

        self.cleanup_expired_transactions();

        let active_count = self.active_transactions.len();
        if active_count >= self.config.max_concurrent_transactions {
            return Err(TransactionError::too_many_transactions());
        }

        let current_write_ts = self.version_manager.next_write_timestamp();
        if snapshot_ts > current_write_ts.saturating_sub(1) {
            return Err(TransactionError::internal(format!(
                "Snapshot timestamp {} is too recent (max: {})",
                snapshot_ts,
                current_write_ts.saturating_sub(1)
            )));
        }

        let txn_id = TransactionId(self.id_generator.fetch_add(1, Ordering::SeqCst));
        let timestamp = self.version_manager.acquire_read_timestamp();
        let timeout = options.timeout.unwrap_or(self.config.default_timeout);

        let config = TransactionConfig {
            timeout,
            durability: DurabilityLevel::Async,
            isolation_level: IsolationLevel::RepeatableRead,
            query_timeout: options.query_timeout,
            statement_timeout: options.statement_timeout,
            idle_timeout: options.idle_timeout,
            two_phase_commit: false,
        };

        let mut context = TransactionContext::new_readonly(txn_id, timestamp, config);
        context.set_snapshot_timestamp(snapshot_ts);

        self.active_transactions.insert(txn_id, Arc::new(context));
        self.stats.record_txn_begin();

        Ok(txn_id)
    }

    /// Start a new insert transaction
    pub fn begin_insert_transaction(
        &self,
        options: TransactionOptions,
    ) -> Result<TransactionId, TransactionError> {
        if self.shutdown_flag.load(Ordering::SeqCst) != 0 {
            return Err(TransactionError::internal(
                "Transaction manager is shutdown".to_string(),
            ));
        }

        self.cleanup_expired_transactions();

        let active_count = self.active_transactions.len();
        if active_count >= self.config.max_concurrent_transactions {
            return Err(TransactionError::too_many_transactions());
        }

        if self.has_active_write_transaction() {
            return Err(TransactionError::write_transaction_conflict());
        }

        let txn_id = TransactionId(self.id_generator.fetch_add(1, Ordering::SeqCst));
        let timestamp = self.version_manager.acquire_insert_timestamp();
        let timeout = options.timeout.unwrap_or(self.config.default_timeout);

        let config = TransactionConfig {
            timeout,
            durability: options.durability,
            isolation_level: options.isolation_level,
            query_timeout: options.query_timeout,
            statement_timeout: options.statement_timeout,
            idle_timeout: options.idle_timeout,
            two_phase_commit: options.two_phase_commit,
        };

        let context = Arc::new(TransactionContext::new(txn_id, timestamp, config));

        self.active_transactions.insert(txn_id, context);
        self.stats.record_txn_begin();

        Ok(txn_id)
    }

    /// Start a new update transaction
    ///
    /// Update transactions require exclusive access and will block
    /// until all other transactions complete.
    pub fn begin_update_transaction(
        &self,
        options: TransactionOptions,
    ) -> Result<TransactionId, TransactionError> {
        if self.shutdown_flag.load(Ordering::SeqCst) != 0 {
            return Err(TransactionError::internal(
                "Transaction manager is shutdown".to_string(),
            ));
        }

        if self.has_active_write_transaction() {
            return Err(TransactionError::write_transaction_conflict());
        }

        let txn_id = TransactionId(self.id_generator.fetch_add(1, Ordering::SeqCst));
        let timestamp = self
            .version_manager
            .acquire_update_timestamp()
            .map_err(|e| TransactionError::internal(e.to_string()))?;
        let timeout = options.timeout.unwrap_or(self.config.default_timeout);

        let config = TransactionConfig {
            timeout,
            durability: options.durability,
            isolation_level: options.isolation_level,
            query_timeout: options.query_timeout,
            statement_timeout: options.statement_timeout,
            idle_timeout: options.idle_timeout,
            two_phase_commit: options.two_phase_commit,
        };

        let context = Arc::new(TransactionContext::new(txn_id, timestamp, config));

        self.active_transactions.insert(txn_id, context);
        self.stats.record_txn_begin();

        Ok(txn_id)
    }

    /// Check for write-set based conflicts with active transactions
    ///
    /// This method checks if a transaction's write set conflicts with any active write transactions.
    /// Returns Ok(()) if no conflicts, or Err if conflicts are detected.
    ///
    /// Note: This is a check-at-call-time method for dynamic conflict detection.
    pub fn check_write_set_conflict(&self, txn_id: TransactionId) -> Result<(), TransactionError> {
        let ctx = self
            .active_transactions
            .get(&txn_id)
            .ok_or_else(|| TransactionError::transaction_not_found(txn_id))?;

        if ctx.read_only {
            return Ok(());
        }

        let txn_write_set = ctx.get_write_set();
        if txn_write_set.is_empty() {
            return Ok(());
        }

        for entry in self.active_transactions.iter() {
            let (other_id, other_ctx) = entry.pair();

            if other_id == &txn_id {
                continue;
            }

            if other_ctx.read_only {
                continue;
            }

            if ctx.has_write_conflict_with(other_ctx) {
                return Err(TransactionError::write_transaction_conflict());
            }
        }

        Ok(())
    }

    /// Start a new transaction (legacy API for compatibility)
    pub fn begin_transaction(
        &self,
        options: TransactionOptions,
    ) -> Result<TransactionId, TransactionError> {
        if options.read_only {
            self.begin_read_transaction(options)
        } else {
            self.begin_insert_transaction(options)
        }
    }

    /// Get transaction context
    pub fn get_context(
        &self,
        txn_id: TransactionId,
    ) -> Result<Arc<TransactionContext>, TransactionError> {
        self.active_transactions
            .get(&txn_id)
            .map(|entry| entry.value().clone())
            .ok_or(TransactionError::transaction_not_found(txn_id))
    }

    /// Check if transaction exists and is active
    pub fn is_transaction_active(&self, txn_id: TransactionId) -> bool {
        self.active_transactions
            .get(&txn_id)
            .map(|entry| entry.value().state().can_execute())
            .unwrap_or(false)
    }

    /// Commit transaction
    ///
    /// Follows atomic commit protocol:
    /// 1. Check state and timeout (transaction still active)
    /// 2. Transition to Committing (marks in-progress, prevents concurrent operations)
    /// 3. Call sync_manager (external coordination) - if this fails, the transaction is terminated
    ///    and resources are released. The current state machine does not support retrying a
    ///    commit from Committing.
    ///    NOTE: sync_manager.commit() is called BEFORE storage-level timestamp release, ensuring that
    ///    storage and index visibility change together.
    /// 4. Release timestamp
    /// 5. Remove from active_transactions (only after all steps succeed)
    /// 6. Transition to Committed
    /// 7. Update stats
    pub fn commit_transaction(&self, txn_id: TransactionId) -> Result<(), TransactionError> {
        let context = {
            let entry = self
                .active_transactions
                .get(&txn_id)
                .ok_or(TransactionError::transaction_not_found(txn_id))?;

            let ctx = entry.value().clone();
            drop(entry);

            if !ctx.state().can_commit() {
                return Err(TransactionError::invalid_state_for_commit(ctx.state()));
            }

            if ctx.is_expired() {
                self.stats.increment_timeout();
                self.rollback_context_timestamp(&ctx);
                self.active_transactions.remove(&txn_id);
                return Err(TransactionError::transaction_timeout());
            }

            ctx
        };

        context.transition_to(TransactionState::Committing)?;

        if let Some(ref sync_manager) = self.sync_manager {
            if let Err(e) = sync_manager.commit_transaction_sync(txn_id) {
                log::warn!(
                    "Sync commit failed for transaction {}, aborting transaction: {}",
                    txn_id,
                    e
                );
                self.rollback_context_timestamp(&context);
                self.active_transactions.remove(&txn_id);
                let _ = context.transition_to(TransactionState::Aborted);
                return Err(TransactionError::sync_failed(format!(
                    "Failed to commit sync data for transaction {}: {}",
                    txn_id, e
                )));
            }
        }

        if context.read_only {
            self.version_manager.release_read_timestamp();
        } else {
            self.version_manager
                .release_insert_timestamp(context.timestamp());
        }

        self.active_transactions.remove(&txn_id);

        context.transition_to(TransactionState::Committed)?;

        self.stats.record_txn_commit();

        Ok(())
    }

    /// Commit transaction with undo target (for rollback support)
    pub fn commit_transaction_with_undo<T: UndoTarget + ?Sized>(
        &self,
        txn_id: TransactionId,
        _target: &mut T,
    ) -> Result<(), TransactionError> {
        self.commit_transaction(txn_id)
    }

    /// Abort transaction
    ///
    /// Follows atomic abort protocol:
    /// 1. Check state (transaction still active)
    /// 2. Transition to Aborting
    /// 3. Call sync_manager rollback. If it fails, the transaction is terminated and resources
    ///    are released.
    /// 4. Release timestamp
    /// 5. Remove from active_transactions (only after all steps succeed)
    /// 6. Transition to Aborted
    /// 7. Update stats
    pub fn abort_transaction(&self, txn_id: TransactionId) -> Result<(), TransactionError> {
        let context = {
            let entry = self
                .active_transactions
                .get(&txn_id)
                .ok_or(TransactionError::transaction_not_found(txn_id))?;
            let ctx = entry.value().clone();
            drop(entry);

            if !ctx.state().can_abort() {
                return Err(TransactionError::invalid_state_for_abort(ctx.state()));
            }

            ctx
        };

        self.abort_transaction_internal(&context)
    }

    /// Abort transaction with undo target (for rollback support)
    ///
    /// Follows atomic abort protocol:
    /// 1. Check state (transaction still active)
    /// 2. Execute undo log rollback
    /// 3. Transition to Aborting
    /// 4. Call sync_manager rollback. If it fails, the transaction is terminated and resources
    ///    are released.
    /// 5. Release timestamp
    /// 6. Remove from active_transactions
    /// 7. Transition to Aborted
    /// 8. Update stats
    pub fn abort_transaction_with_undo<T: UndoTarget + ?Sized>(
        &self,
        txn_id: TransactionId,
        target: &mut T,
    ) -> Result<(), TransactionError> {
        let context = {
            let entry = self
                .active_transactions
                .get(&txn_id)
                .ok_or(TransactionError::transaction_not_found(txn_id))?;
            let ctx = entry.value().clone();
            drop(entry);

            if !ctx.state().can_abort() {
                return Err(TransactionError::invalid_state_for_abort(ctx.state()));
            }

            ctx
        };

        // Execute undo log rollback first (before state transition)
        let rollback = UndoLogRollback::new(&*context);
        rollback
            .execute_rollback(target, context.timestamp())
            .map_err(|e| TransactionError::rollback_failed(e.to_string()))?;
        rollback.clear_logs();

        self.abort_transaction_internal(&context)
    }

    /// Internal abort implementation
    ///
    /// Atomic abort protocol:
    /// 1. Transition to Aborting (marks in-progress)
    /// 2. Call sync_manager rollback. If it fails, the transaction is terminated and resources
    ///    are released.
    /// 3. Release timestamp
    /// 4. Remove from active_transactions (only after all steps succeed)
    /// 5. Transition to Aborted
    /// 6. Update stats
    fn abort_transaction_internal(
        &self,
        context: &TransactionContext,
    ) -> Result<(), TransactionError> {
        context.transition_to(TransactionState::Aborting)?;

        if let Some(ref sync_manager) = self.sync_manager {
            if let Err(e) = sync_manager.rollback_transaction_sync(context.id) {
                log::warn!(
                    "Sync rollback failed for transaction {}, aborting transaction: {}",
                    context.id,
                    e
                );
                self.rollback_context_timestamp(context);
                self.active_transactions.remove(&context.id);
                let _ = context.transition_to(TransactionState::Aborted);
                return Err(TransactionError::sync_failed(format!(
                    "Failed to rollback sync data for transaction {}: {}",
                    context.id, e
                )));
            }
        }

        if context.read_only {
            self.version_manager.release_read_timestamp();
        } else {
            self.version_manager
                .release_insert_timestamp(context.timestamp());
        }

        self.active_transactions.remove(&context.id);

        context.transition_to(TransactionState::Aborted)?;

        self.stats.record_txn_rollback();

        Ok(())
    }

    fn rollback_context_timestamp(&self, context: &TransactionContext) {
        if context.read_only {
            self.version_manager.release_read_timestamp();
        } else {
            self.version_manager
                .release_insert_timestamp(context.timestamp());
        }
    }

    /// Get active transaction list
    pub fn list_active_transactions(&self) -> Vec<TransactionInfo> {
        self.monitor
            .list_active_transactions(&self.active_transactions)
    }

    /// Get transaction info
    pub fn get_transaction_info(&self, txn_id: TransactionId) -> Option<TransactionInfo> {
        self.monitor
            .get_transaction_info(&self.active_transactions, txn_id)
    }

    /// Get statistics
    pub fn stats(&self) -> &TransactionStats {
        self.monitor.stats()
    }

    /// Cleanup expired transactions
    pub fn cleanup_expired_transactions(&self) {
        self.cleaner
            .cleanup_expired_transactions(&self.active_transactions);
    }

    /// Shutdown transaction manager
    pub fn shutdown(&self) {
        self.shutdown_flag.store(1, Ordering::SeqCst);

        let txn_ids: Vec<TransactionId> = {
            self.active_transactions
                .iter()
                .map(|entry| *entry.key())
                .collect()
        };

        for txn_id in txn_ids {
            let _ = self.abort_transaction(txn_id);
        }
    }

    /// Get configuration
    pub fn config(&self) -> TransactionManagerConfig {
        self.config.clone()
    }

    /// Create savepoint
    pub fn create_savepoint(
        &self,
        txn_id: TransactionId,
        name: Option<String>,
    ) -> Result<SavepointId, TransactionError> {
        let context = self.get_context(txn_id)?;
        let sync_sequence = self
            .sync_manager
            .as_ref()
            .map(|manager| manager.sync_sequence(txn_id))
            .unwrap_or(0);
        Ok(context.create_savepoint(name, sync_sequence))
    }

    /// Get savepoint info
    pub fn get_savepoint(&self, txn_id: TransactionId, id: SavepointId) -> Option<SavepointInfo> {
        let context = self.get_context(txn_id).ok()?;
        context.get_savepoint(id)
    }

    /// Release savepoint
    pub fn release_savepoint(
        &self,
        txn_id: TransactionId,
        id: SavepointId,
    ) -> Result<(), TransactionError> {
        let context = self.get_context(txn_id)?;
        context.release_savepoint(id)
    }

    /// Rollback to savepoint
    pub fn rollback_to_savepoint<T: UndoTarget + ?Sized>(
        &self,
        txn_id: TransactionId,
        id: SavepointId,
        target: &T,
    ) -> Result<(), TransactionError> {
        let context = self.get_context(txn_id)?;
        let savepoint = context
            .get_savepoint(id)
            .ok_or(TransactionError::savepoint_not_found(id))?;

        if let Some(sync_manager) = self.sync_manager.as_ref() {
            sync_manager
                .rollback_transaction_to_sequence_sync(txn_id, savepoint.sync_sequence)
                .map_err(|e| TransactionError::sync_failed(e.to_string()))?;
        }

        context
            .rollback_to_savepoint(id, target)
            .map_err(|e| TransactionError::rollback_failed(e.to_string()))?;

        Ok(())
    }

    /// Get all active savepoints for transaction
    pub fn get_active_savepoints(&self, txn_id: TransactionId) -> Vec<SavepointInfo> {
        self.get_context(txn_id)
            .map(|ctx| ctx.get_all_savepoints())
            .unwrap_or_default()
    }

    /// Get current write timestamp
    pub fn write_timestamp(&self) -> u32 {
        self.version_manager.write_timestamp()
    }

    /// Get current read timestamp
    pub fn read_timestamp(&self) -> u32 {
        self.version_manager.read_timestamp()
    }

    /// Check if an update transaction is in progress
    pub fn is_update_in_progress(&self) -> bool {
        self.version_manager.is_update_in_progress()
    }

    /// Get pending transaction count
    pub fn pending_count(&self) -> i32 {
        self.version_manager.pending_count()
    }

    /// Check if there's an active write transaction
    fn has_active_write_transaction(&self) -> bool {
        self.active_transactions
            .iter()
            .any(|entry| !entry.value().read_only)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{
        ColumnId, EdgeDeletionContext, EdgeIdentifier, EdgeKey, VertexIdentifier,
    };
    use crate::transaction::undo_log::{PropertyValue, UndoLogResult, UndoTarget};

    #[test]
    fn test_transaction_manager_basic() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());

        let txn_id = manager
            .begin_read_transaction(TransactionOptions::default())
            .expect("Failed to begin read transaction");

        assert!(manager.is_transaction_active(txn_id));

        manager
            .commit_transaction(txn_id)
            .expect("Failed to commit");

        assert!(!manager.is_transaction_active(txn_id));
    }

    #[test]
    fn test_transaction_manager_insert() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());

        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("Failed to begin insert transaction");

        assert!(manager.is_transaction_active(txn_id));

        manager
            .commit_transaction(txn_id)
            .expect("Failed to commit");

        assert!(!manager.is_transaction_active(txn_id));
    }

    #[test]
    fn test_transaction_manager_abort() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());

        let txn_id = manager
            .begin_read_transaction(TransactionOptions::default())
            .expect("Failed to begin read transaction");

        manager.abort_transaction(txn_id).expect("Failed to abort");

        assert!(!manager.is_transaction_active(txn_id));
        assert_eq!(
            manager.stats().aborted_transactions.load(Ordering::Relaxed),
            1
        );
    }

    #[test]
    fn test_transaction_manager_savepoint() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());

        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("Failed to begin transaction");

        let sp_id = manager
            .create_savepoint(txn_id, Some("test".to_string()))
            .expect("Failed to create savepoint");

        let sp = manager
            .get_savepoint(txn_id, sp_id)
            .expect("Failed to get savepoint");
        assert_eq!(sp.name, Some("test".to_string()));

        manager
            .commit_transaction(txn_id)
            .expect("Failed to commit");
    }

    #[test]
    fn test_transaction_manager_shutdown() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());

        let txn_id = manager
            .begin_read_transaction(TransactionOptions::default())
            .expect("Failed to begin transaction");

        manager.shutdown();

        assert!(!manager.is_transaction_active(txn_id));
    }

    #[test]
    fn test_transaction_manager_with_sync_manager() {
        use crate::sync::SyncManager;

        let sync_manager = Arc::new(SyncManager::new_without_fulltext());
        let manager = TransactionManager::new(TransactionManagerConfig::default())
            .with_sync_manager(sync_manager);

        assert!(manager.sync_manager.is_some());
    }

    #[test]
    fn test_rollback_to_savepoint_with_sync_manager() {
        use crate::sync::SyncManager;

        struct MockUndoTarget;
        impl UndoTarget for MockUndoTarget {
            fn delete_vertex_type(&self, _label: crate::transaction::LabelId) -> UndoLogResult<()> {
                Ok(())
            }
            fn delete_edge_type(&self, _edge_key: EdgeKey) -> UndoLogResult<()> {
                Ok(())
            }
            fn delete_vertex(
                &self,
                _vertex: VertexIdentifier,
                _ts: crate::transaction::Timestamp,
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn delete_edge(&self, _edge_ctx: EdgeDeletionContext) -> UndoLogResult<()> {
                Ok(())
            }
            fn undo_update_vertex_property(
                &self,
                _vertex: VertexIdentifier,
                _col_id: ColumnId,
                _value: PropertyValue,
                _ts: crate::transaction::Timestamp,
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn undo_update_edge_property(
                &self,
                _edge_id: EdgeIdentifier,
                _oe_offset: i32,
                _ie_offset: i32,
                _col_id: ColumnId,
                _value: PropertyValue,
                _ts: crate::transaction::Timestamp,
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn revert_delete_vertex(
                &self,
                _vertex: VertexIdentifier,
                _ts: crate::transaction::Timestamp,
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn revert_delete_edge(&self, _edge_ctx: EdgeDeletionContext) -> UndoLogResult<()> {
                Ok(())
            }
            fn revert_delete_vertex_properties(
                &self,
                _label_name: &str,
                _prop_names: &[String],
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn revert_delete_edge_properties(
                &self,
                _src_label: &str,
                _dst_label: &str,
                _edge_label: &str,
                _prop_names: &[String],
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn revert_delete_vertex_label(&self, _label_name: &str) -> UndoLogResult<()> {
                Ok(())
            }
            fn revert_delete_edge_label(
                &self,
                _src_label: &str,
                _dst_label: &str,
                _edge_label: &str,
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn revert_rename_vertex_properties(
                &self,
                _label_name: &str,
                _current_names: &[String],
                _original_names: &[String],
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn revert_rename_edge_properties(
                &self,
                _src_label: &str,
                _dst_label: &str,
                _edge_label: &str,
                _current_names: &[String],
                _original_names: &[String],
            ) -> UndoLogResult<()> {
                Ok(())
            }
        }

        let sync_manager = Arc::new(SyncManager::new_without_fulltext());
        let manager = TransactionManager::new(TransactionManagerConfig::default())
            .with_sync_manager(sync_manager);

        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("Failed to begin transaction");
        let sp_id = manager
            .create_savepoint(txn_id, Some("sp".to_string()))
            .expect("Failed to create savepoint");

        let dummy = MockUndoTarget;
        let result = manager.rollback_to_savepoint(txn_id, sp_id, &dummy);
        // rollback_to_savepoint now succeeds as sync_manager properly handles the operation
        assert!(result.is_ok());
    }
}
