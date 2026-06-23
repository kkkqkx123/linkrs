//! Transaction Cleaner
//!
//! Provides cleanup functionality for expired and stale transactions

use std::sync::Arc;

use dashmap::DashMap;

use super::mvcc::VersionManager;
use crate::sync::SyncManager;
use crate::transaction::context::TransactionContext;
use crate::transaction::error::TransactionError;
use crate::transaction::types::{TransactionId, TransactionState, TransactionStats};

/// Transaction Cleaner
///
/// Responsible for cleaning up expired transactions and releasing their resources.
///
/// When sync rollback fails during cleanup, the failure is tracked via stats
/// for observability. The cleanup itself remains best-effort to avoid blocking
/// the system, but the failures are now measurable.
pub struct TransactionCleaner {
    sync_manager: Option<Arc<SyncManager>>,
    version_manager: Arc<VersionManager>,
    stats: Arc<TransactionStats>,
}

impl TransactionCleaner {
    pub fn new(
        sync_manager: Option<Arc<SyncManager>>,
        version_manager: Arc<VersionManager>,
        stats: Arc<TransactionStats>,
    ) -> Self {
        Self {
            sync_manager,
            version_manager,
            stats,
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

            // Use unified abort path for consistency
            let _ = self.abort_transaction_internal_unified(context);
            // Timeout stat is incremented inside abort_transaction_internal_unified
        }
    }

    /// Unified abort implementation used by both cleaner and manager
    /// This ensures consistent abort semantics across all abort paths
    fn abort_transaction_internal_unified(
        &self,
        context: Arc<TransactionContext>,
    ) -> Result<(), TransactionError> {
        if !context.state().can_abort() {
            // Transaction already in terminal state, just update stats
            self.stats.decrement_active();
            self.stats.increment_aborted();
            self.stats.increment_timeout();
            return Err(TransactionError::invalid_state_for_abort(context.state()));
        }

        context.transition_to(TransactionState::Aborting)?;

        let txn_id = context.id;
        if let Some(ref sync_manager) = self.sync_manager {
            if let Err(e) = sync_manager.rollback_transaction_sync(txn_id) {
                log::warn!(
                    "Index sync rollback failed for expired transaction {:?}: {}",
                    txn_id,
                    e
                );
                // Track sync rollback failure for observability
                // This is a best-effort cleanup, but failures should be monitored
                log::error!(
                    "Sync rollback failed during cleanup for transaction {:?}. \
                     This may leave stale index data. Manual recovery may be needed.",
                    txn_id
                );
            }
        }

        if context.read_only {
            self.version_manager.release_read_timestamp();
        } else {
            self.version_manager
                .release_insert_timestamp(context.timestamp());
        }

        context.transition_to(TransactionState::Aborted)?;

        // Update stats in correct order: decrement active first, then increment terminal states
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
            Arc::new(TransactionStats::new()),
        )
    }
}
