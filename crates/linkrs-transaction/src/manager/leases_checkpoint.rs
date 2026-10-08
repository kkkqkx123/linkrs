//! Transaction manager behavior: write leases, checkpoint offers, cleanup and shutdown

use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::Duration;

use super::TransactionManager;
use crate::checkpoint::CheckpointGate;
use crate::context::TransactionContext;
use crate::types::*;
impl TransactionManager {
    /// Start the lifecycle service that removes expired or idle transactions.
    ///
    /// The worker is intentionally opt-in at the manager boundary so embedded
    /// users can keep deterministic lifecycle control in tests and tools.
    pub fn start_auto_cleanup_task(self: &Arc<Self>) -> Option<std::thread::JoinHandle<()>> {
        if !self.config.auto_cleanup {
            return None;
        }

        let manager = Arc::downgrade(self);
        Some(std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(250));
            let Some(manager) = Weak::upgrade(&manager) else {
                break;
            };
            if manager.shutdown_flag.load(Ordering::SeqCst) != 0 {
                break;
            }
            manager.cleanup_expired_transactions();
        }))
    }

    pub(crate) fn maybe_cleanup_expired_transactions(&self) {
        if self.config.auto_cleanup {
            self.cleanup_expired_transactions();
        }
    }

    /// Exponential backoff delay: 100ms * 2^attempt, capped at 10s.
    pub(crate) fn backoff_delay(attempt: u32) -> std::time::Duration {
        let ms = 100u64.saturating_mul(2u64.pow(attempt.min(10)));
        std::time::Duration::from_millis(ms.min(10_000))
    }

    /// Get the checkpoint gate for external coordination.
    pub fn checkpoint_gate(&self) -> &Arc<CheckpointGate> {
        &self.checkpoint_gate
    }

    /// Release a write transaction's manager-owned leases exactly once.
    ///
    /// Single release point shared by the commit path (lease released before
    /// storage finalization so checkpoints do not wait for it) and every
    /// abort path. The one-way `mark_resources_released` flag makes a second
    /// call a no-op: without it, a commit that released the gate and then
    /// failed finalization (or commit-timestamp allocation) would release the
    /// gate a second time when the still-`Committing` transaction is aborted,
    /// underflowing the gate counter and wedging checkpoint drain until
    /// timeout. Timestamp retirement needs no such guard — retiring an
    /// already-terminal slot is a no-op by state check.
    ///
    /// Returns true if this call performed the release.
    pub(crate) fn release_write_lease(&self, context: &Arc<TransactionContext>) -> bool {
        if !context.mark_resources_released() {
            return false;
        }
        if context.txn_type != TransactionType::Write {
            return true;
        }
        if context.has_pessimistic_lock() {
            self.write_exclusion_owner.store(0, Ordering::SeqCst);
        }
        if !self.config.in_memory {
            self.checkpoint_gate.release_write();
        }
        true
    }

    /// Release the single-writer exclusion when its owner is gone.
    ///
    /// The exclusion is an ownership flag, not a scope guard: if the owner
    /// transaction crashed or was reaped without running the release path,
    /// new writers would wedge forever. The cleaner calls this after every
    /// sweep; a live owner is never disturbed.
    fn release_dead_write_owner(&self) {
        let owner = self.write_exclusion_owner.load(Ordering::SeqCst);
        if owner == 0 {
            return;
        }
        let alive = self
            .active_transactions
            .get(&TransactionId(owner))
            .is_some_and(|entry| {
                entry.value().has_pessimistic_lock() && entry.value().state().can_execute()
            });
        if !alive
            && self
                .write_exclusion_owner
                .compare_exchange(owner, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            log::warn!(
                "Released single-writer exclusion held by dead owner {}",
                owner
            );
        }
    }

    /// Whether a checkpoint should be triggered after the latest commit.
    ///
    /// Threshold-based (commit count since the last checkpoint) instead of a
    /// plain boolean: with the default threshold of 1 the behavior matches
    /// the legacy `auto_checkpoint_after_commit` flag (checkpoint offered
    /// after every write commit); higher thresholds batch the offers. The
    /// decision is only an offer — the commit sink decides via
    /// `auto_checkpoint_if_needed` whether its WAL pressure warrants one.
    /// WAL-byte pressure itself lives in the storage layer
    /// (`should_checkpoint` on WAL size), not here: this counter only
    /// throttles how often the offer is made. In-memory mode never checkpoints.
    pub fn should_auto_checkpoint(&self) -> bool {
        if !self.config.auto_checkpoint_after_commit || self.config.in_memory {
            return false;
        }
        let threshold = self.config.auto_checkpoint_commit_threshold.max(1);
        self.commits_since_checkpoint.load(Ordering::Relaxed) >= threshold
    }

    /// Reset the commits-since-checkpoint counter (called when a checkpoint
    /// begins).
    pub(crate) fn reset_checkpoint_commit_counter(&self) {
        self.commits_since_checkpoint.store(0, Ordering::Relaxed);
    }

    /// Number of `Active` write transactions currently in the table.
    ///
    /// Together with `CheckpointGate::active_write_count` this is the
    /// observable pair for lease-leak tests: at quiescence (no in-flight
    /// lifecycle transitions) both must be zero. They are intentionally not
    /// asserted against each other on lifecycle paths because acquire and
    /// table insertion (or state transition and release) cannot be atomic
    /// under concurrent commits.
    pub fn active_write_count(&self) -> usize {
        self.active_transactions
            .iter()
            .filter(|entry| {
                entry.value().txn_type == TransactionType::Write
                    && entry.value().state() == TransactionState::Active
            })
            .count()
    }

    /// Cleanup expired transactions
    pub fn cleanup_expired_transactions(&self) {
        self.cleaner
            .cleanup_expired_transactions_with(&self.active_transactions, |txn_id| {
                self.abort_transaction(txn_id)
            });
        self.release_dead_write_owner();
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
            // Committed transactions are terminal and must never be aborted;
            // they normally leave the table on commit, this is defense in depth.
            if let Some(entry) = self.active_transactions.get(&txn_id) {
                if entry.value().state() == TransactionState::Committed {
                    continue;
                }
            }
            if let Err(error) = self.abort_transaction(txn_id) {
                log::error!(
                    "Abort failed for transaction {} during shutdown: {}",
                    txn_id,
                    error
                );
                self.stats.increment_cleanup_failure();
            }
        }
    }
}
