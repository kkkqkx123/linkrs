//! Transaction Cleaner
//!
//! Provides cleanup functionality for expired and stale transactions

use std::collections::HashSet;
use std::sync::Arc;

use dashmap::DashMap;

use super::mvcc::VersionManager;
use crate::context::TransactionContext;
use crate::error::TransactionError;
use crate::types::{TransactionId, TransactionStats};
use graphdb_core::types::Timestamp;

/// Transaction Cleaner
///
/// Provides cleanup functionality for expired and stale transactions
///
/// Expiry detection and timestamp reaping live here; the abort itself always
/// runs through the manager's canonical abort protocol (via the callback in
/// `cleanup_expired_transactions_with`), so abort semantics cannot diverge
/// between cleanup and explicit rollback paths.
pub struct TransactionCleaner {
    version_manager: Arc<VersionManager>,
    stats: Arc<TransactionStats>,
}

impl TransactionCleaner {
    pub fn new(version_manager: Arc<VersionManager>, stats: Arc<TransactionStats>) -> Self {
        Self {
            version_manager,
            stats,
        }
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
}

impl Default for TransactionCleaner {
    fn default() -> Self {
        Self::new(
            Arc::new(VersionManager::new()),
            Arc::new(TransactionStats::new()),
        )
    }
}
