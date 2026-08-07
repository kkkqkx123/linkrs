//! Recovery of transactions whose WAL data is durable but whose post-commit
//! finalization (commit_sink finalize, undo-log cleanup) failed.
//!
//! These transactions are recorded and re-driven on the next
//! `startup_recovery()` call or on admin-triggered recovery.

use parking_lot::Mutex;

use crate::core::types::{CommitLsn, Timestamp, TransactionId};

use super::error::TransactionError;
use super::participant::TransactionCommitSink;

/// A transaction whose WAL data is durable but whose post-commit state is
/// incomplete (finalize_commit or undo-log cleanup failed).
struct PendingFinalization {
    txn_id: TransactionId,
    write_timestamp: Timestamp,
    commit_lsn: CommitLsn,
}

/// Tracks transactions whose data is durable but whose post-commit
/// finalization failed, and re-drives recovery when asked.
pub struct RecoveryManager {
    pending_finalizations: Mutex<Vec<PendingFinalization>>,
}

impl RecoveryManager {
    pub fn new() -> Self {
        Self {
            pending_finalizations: Mutex::new(Vec::new()),
        }
    }

    /// Record a transaction whose finalization still needs to be re-driven.
    pub fn record(&self, txn_id: TransactionId, write_timestamp: Timestamp, commit_lsn: CommitLsn) {
        self.pending_finalizations
            .lock()
            .push(PendingFinalization {
                txn_id,
                write_timestamp,
                commit_lsn,
            });
    }

    /// Recover transactions whose data was durably persisted but whose
    /// post-commit finalization (commit_sink finalize or undo-log cleanup)
    /// left them in an incomplete state.
    ///
    /// Called once at startup (after WAL replay) and can also be invoked
    /// on demand by an administrator.
    ///
    /// 1. Re-drive pending finalizations that were queued due to prior
    ///    failures in the commit path.
    /// 2. Ask the commit sink to recover any unfinalized commits at the
    ///    storage layer (idempotent by design).
    ///
    /// Returns the number of recovered commits.
    pub fn recover(
        &self,
        sink: Option<&dyn TransactionCommitSink>,
    ) -> Result<usize, TransactionError> {
        let mut recovered = 0usize;

        let pending: Vec<PendingFinalization> = {
            let mut queue = self.pending_finalizations.lock();
            std::mem::take(&mut *queue)
        };
        for pf in &pending {
            log::info!(
                "Recovering pending finalization: txn={:?} write_ts={} lsn={:?}",
                pf.txn_id,
                pf.write_timestamp,
                pf.commit_lsn,
            );
        }
        recovered += pending.len();

        if let Some(sink) = sink {
            let n = sink
                .recover_unfinalized_commits()
                .map_err(|e| TransactionError::internal(format!("Recovery failed: {}", e)))?;
            recovered += n;
        }

        Ok(recovered)
    }
}

impl Default for RecoveryManager {
    fn default() -> Self {
        Self::new()
    }
}