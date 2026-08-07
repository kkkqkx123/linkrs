//! Transaction Cleaner
//!
//! Provides cleanup functionality for expired and stale transactions

use std::collections::HashSet;
use std::sync::Arc;

use dashmap::DashMap;

use super::checkpoint::CheckpointGate;
use super::mvcc::VersionManager;
use crate::sync::SyncManager;
use crate::transaction::context::TransactionContext;
use crate::transaction::error::TransactionError;
use crate::transaction::types::{TransactionId, TransactionState, TransactionStats};
use crate::core::types::Timestamp;

/// Transaction Cleaner
///
/// Provides cleanup functionality for expired and stale transactions
///
/// When sync rollback fails during cleanup, the failure is tracked via stats
/// for observability. The cleanup itself remains best-effort to avoid blocking
/// the system, but the failures are now measurable.
pub struct TransactionCleaner {
    sync_manager: Option<Arc<SyncManager>>,
    version_manager: Arc<VersionManager>,
    checkpoint_gate: Arc<CheckpointGate>,
    stats: Arc<TransactionStats>,
}

impl TransactionCleaner {
    pub fn new(
        sync_manager: Option<Arc<SyncManager>>,
        version_manager: Arc<VersionManager>,
        checkpoint_gate: Arc<CheckpointGate>,
        stats: Arc<TransactionStats>,
    ) -> Self {
        Self {
            sync_manager,
            version_manager,
            checkpoint_gate,
            stats,
        }
    }

    /// Update the sync manager after construction (e.g. when a manager is
    /// assembled incrementally by its owner).
    pub fn set_sync_manager(&mut self, sync_manager: Option<Arc<SyncManager>>) {
        self.sync_manager = sync_manager;
    }

    /// Cleanup expired transactions, delegating the abort to the provided
    /// callback.
    ///
    /// This is the manager-facing cleanup path. It reaps orphaned write
    /// timestamps whose owning path vanished, then aborts every expired or
    /// idle-timeout transaction through the callback so the caller keeps its
    /// canonical abort semantics (full protocol, sink, events).
    pub fn cleanup_expired_transactions_with<F>(
        &self,
        active_transactions: &DashMap<TransactionId, Arc<TransactionContext>>,
        mut abort: F,
    ) where
        F: FnMut(TransactionId) -> Result<(), TransactionError>,
    {
        // Safety net: reap write timestamps whose owning path vanished (orphaned
        // write). Timestamps owned by live write transactions are excluded so a
        // transaction still within its configured timeout is never reaped.
        let owned: HashSet<Timestamp> = active_transactions
            .iter()
            .filter(|entry| !entry.value().read_only)
            .map(|entry| entry.value().timestamp())
            .collect();
        let reaped = self
            .version_manager
            .reap_expired_write_timestamps(self.version_manager.write_reap_timeout(), &owned);
        if reaped > 0 {
            log::warn!("Reaped {reaped} orphaned write timestamp(s) older than timeout");
        }

        let expired: Vec<(TransactionId, bool)> = active_transactions
            .iter()
            .filter(|entry| {
                entry.value().state().can_execute()
                    && (entry.value().is_expired() || entry.value().is_idle_timeout())
            })
            .map(|entry| (*entry.key(), entry.value().is_expired()))
            .collect();

        for (txn_id, timed_out) in expired {
            if timed_out {
                self.stats.record_timeout();
            }
            if let Err(error) = abort(txn_id) {
                log::error!(
                    "Transaction {} could not complete the cleanup protocol: {}",
                    txn_id,
                    error
                );
                self.stats.increment_cleanup_failure();
            }
        }
    }

    /// Cleanup expired transactions
    ///
    /// This method removes all expired transactions and releases their resources.
    /// It should be called periodically or before starting new write transactions
    /// to prevent stale transactions from blocking operations.
    ///
    /// Uses the same abort protocol as normal abort to ensure consistency:
    /// 1. Remove from active_transactions
    /// 2. Transition to Aborting
    /// 3. Call sync_manager rollback (errors logged but don't fail cleanup)
    /// 4. Release timestamp
    /// 5. Transition to Aborted
    /// 6. Update stats (decrement_active, increment_aborted, increment_timeout)
    pub fn cleanup_expired_transactions(
        &self,
        active_transactions: &DashMap<TransactionId, Arc<TransactionContext>>,
    ) {
        let expired: Vec<TransactionId> = {
            active_transactions
                .iter()
                .filter(|entry| entry.value().is_expired())
                .map(|entry| *entry.key())
                .collect()
        };

        if expired.is_empty() {
            return;
        }

        log::debug!("Cleaning up {} expired transactions", expired.len());

        for txn_id in expired {
            let context = {
                if let Some((_, ctx)) = active_transactions.remove(&txn_id) {
                    ctx
                } else {
                    continue;
                }
            };

            if let Err(error) = self.abort_transaction_internal_unified(context) {
                log::error!(
                    "Cleanup failed to abort expired transaction {:?}: {}",
                    txn_id,
                    error
                );
                self.stats.increment_cleanup_failure();
            }
        }
    }

    /// Unified abort implementation used by both cleaner and manager
    /// This ensures consistent abort semantics across all abort paths
    fn abort_transaction_internal_unified(
        &self,
        context: Arc<TransactionContext>,
    ) -> Result<(), TransactionError> {
        if !context.state().can_abort() {
            self.stats.decrement_active();
            self.stats.increment_aborted();
            self.stats.increment_timeout();
            return Err(TransactionError::invalid_state_for_abort(context.state()));
        }

        context.transition_to(TransactionState::Aborting)?;

        let txn_id = context.id;
        if let Some(ref sync_manager) = self.sync_manager {
            if let Err(e) = sync_manager.rollback_transaction_sync(txn_id) {
                log::error!(
                    "Index sync rollback failed for expired transaction {:?}: {}",
                    txn_id,
                    e
                );
                log::error!(
                    "Sync rollback failed during cleanup for transaction {:?}. \
                     This may leave stale index data. Manual recovery may be needed.",
                    txn_id
                );
            }
        }

        if context.read_only {
            self.version_manager
                .release_read_timestamp_at(context.start_timestamp);
        } else {
            self.version_manager
                .abort_write_timestamp(context.timestamp());
            self.checkpoint_gate.release_write();
        }

        context.transition_to(TransactionState::Aborted)?;

        self.stats.decrement_active();
        self.stats.increment_aborted();
        self.stats.increment_timeout();

        Ok(())
    }

    /// Abort transaction by ID (helper for cleanup operations)
    ///
    /// Uses the unified abort path for consistency with normal abort.
    pub fn abort_transaction_by_id(
        &self,
        active_transactions: &DashMap<TransactionId, Arc<TransactionContext>>,
        txn_id: TransactionId,
    ) -> Result<(), TransactionError> {
        let context = active_transactions
            .remove(&txn_id)
            .map(|(_, ctx)| ctx)
            .ok_or(TransactionError::transaction_not_found(txn_id))?;

        self.abort_transaction_internal_unified(context)
    }
}

impl Default for TransactionCleaner {
    fn default() -> Self {
        Self::new(
            None,
            Arc::new(VersionManager::new()),
            Arc::new(CheckpointGate::new()),
            Arc::new(TransactionStats::new()),
        )
    }
}
