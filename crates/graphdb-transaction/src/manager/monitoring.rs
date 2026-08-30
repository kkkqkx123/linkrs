//! Transaction monitoring and statistics

use std::time::Duration;

use super::TransactionManager;
use crate::error::TransactionError;
use crate::types::*;

impl TransactionManager {
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

    /// List active and recovery-required transactions for administration.
    pub fn list_transactions(&self) -> Vec<TransactionInfo> {
        self.monitor.list_transactions(&self.active_transactions)
    }

    /// Retry delivery of pending synchronization outbox entries.
    pub fn retry_outbox_projection(&self) -> Result<usize, TransactionError> {
        self.sync_manager
            .as_ref()
            .ok_or_else(|| TransactionError::sync_failed("Sync manager is not configured"))?
            .retry_outbox_sync()
            .map_err(|error| TransactionError::sync_failed(error.to_string()))
    }

    /// Recover transactions whose data was durably persisted but whose
    /// post-commit finalization (commit_sink finalize or undo-log cleanup)
    /// left them in an incomplete state.
    ///
    /// Called once at startup (after WAL replay) and can also be invoked
    /// on demand by an administrator.
    ///
    /// Returns the number of recovered commits.
    pub fn startup_recovery(&self) -> Result<usize, TransactionError> {
        self.recovery.recover(self.commit_sink.as_deref())
    }

    /// Get statistics
    pub fn stats(&self) -> &TransactionStats {
        self.monitor.stats()
    }

    /// Return resource gauges used by monitoring and administrative tooling.
    pub fn resource_metrics(&self) -> TransactionResourceMetrics {
        let mut staged_wal_bytes = 0;
        let mut undo_bytes = 0;
        for entry in self.active_transactions.iter() {
            let context = entry.value();
            staged_wal_bytes += context.staged_bytes();
            undo_bytes += context.undo_log_len() as u64;
        }

        let metrics = TransactionResourceMetrics {
            active_snapshots: self.version_manager.snapshot_tracker().active_count() as u64,
            pending_writes: self.version_manager.pending_count(),
            committed_frontier_lag: self
                .version_manager
                .write_timestamp()
                .saturating_sub(self.version_manager.read_timestamp()),
            staged_wal_bytes,
            undo_bytes,
            checkpoint_drain_time: Duration::ZERO,
        };
        self.stats.record_resource_metrics(metrics);
        metrics
    }
}
