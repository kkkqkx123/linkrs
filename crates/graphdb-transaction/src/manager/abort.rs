//! Transaction abort protocol

use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::{rollback_context_timestamp, TransactionManager};
use crate::context::TransactionContext;
use crate::error::TransactionError;
use crate::participant::TransactionAbortDescriptor;
use crate::rollback::UndoLogRollback;
use crate::types::*;
use crate::undo_log::UndoTarget;

impl TransactionManager {
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
    /// 2. Transition to Aborting
    /// 3. Execute undo log rollback
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

        // The commit sink owns the storage undo target. Executing it here as
        // well would apply every undo entry twice.
        context.transition_to(TransactionState::Aborting)?;

        if self.commit_sink.is_none() {
            let rollback = UndoLogRollback::new(&*context);
            if let Err(error) = rollback.execute_rollback(target, context.timestamp()) {
                let _ = self.execute_abort_internal(&context);
                return Err(TransactionError::rollback_failed(error.to_string()));
            }
            rollback
                .clear_logs()
                .map_err(|error| TransactionError::rollback_failed(error.to_string()))?;
        }

        self.execute_abort_internal(&context)
    }

    /// Internal abort implementation.
    ///
    /// Atomic abort protocol:
    /// 1. Transition to Aborting (marks in-progress)
    /// 2. Call sync_manager rollback. If it fails, the transaction is terminated and resources
    ///    are released.
    /// 3. Release timestamp
    /// 4. Remove from active_transactions (only after all steps succeed)
    /// 5. Transition to Aborted
    /// 6. Update stats
    pub(super) fn abort_transaction_internal(
        &self,
        context: &Arc<TransactionContext>,
    ) -> Result<(), TransactionError> {
        context.transition_to(TransactionState::Aborting)?;
        self.execute_abort_internal(context)
    }

    /// Execute abort steps (transition already done by caller).
    fn execute_abort_internal(
        &self,
        context: &Arc<TransactionContext>,
    ) -> Result<(), TransactionError> {
        let max_retries = self.config.abort_retry_attempts;

        if context.txn_type != TransactionType::Checkpoint {
            if let Some(ref commit_sink) = self.commit_sink {
                let mut last_error = None;
                for attempt in 0..=max_retries {
                    let descriptor = TransactionAbortDescriptor {
                        transaction_id: context.id,
                        write_timestamp: context.timestamp(),
                        context: Arc::clone(context),
                    };
                    match commit_sink.abort_transaction_with_descriptor(&descriptor) {
                        Ok(_) => {
                            last_error = None;
                            break;
                        }
                        Err(e) => {
                            last_error = Some(e);
                            if attempt < max_retries {
                                std::thread::sleep(Self::backoff_delay(attempt));
                            }
                        }
                    }
                }
                if let Some(err) = last_error {
                    self.finalize_resources_after_sink_failure(context);
                    return Err(TransactionError::rollback_failed(format!(
                        "Failed to discard transaction {} persistence state after {} retries: {}",
                        context.id, max_retries, err
                    )));
                }
            } else if let Some(ref sync_manager) = self.sync_manager {
                let mut last_error = None;
                for attempt in 0..=max_retries {
                    match sync_manager.rollback_transaction_sync(context.id) {
                        Ok(_) => {
                            last_error = None;
                            break;
                        }
                        Err(e) => {
                            last_error = Some(e);
                            if attempt < max_retries {
                                std::thread::sleep(Self::backoff_delay(attempt));
                            }
                        }
                    }
                }
                if let Some(e) = last_error {
                    log::warn!(
                        "Sync rollback failed for transaction {} after {} retries, aborting: {}",
                        context.id,
                        max_retries,
                        e
                    );
                    self.finalize_resources_after_sink_failure(context);
                    return Err(TransactionError::sync_failed(format!(
                        "Failed to rollback sync data for transaction {} after {} retries: {}",
                        context.id, max_retries, e
                    )));
                }
            }
        }

        self.finalize_abort_resources(context);

        let safe_ts = self.version_manager.get_safe_gc_timestamp();
        self.prune_committed_write_sets(safe_ts);

        Ok(())
    }

    /// Release every manager-owned resource exactly once after abort, even if
    /// persistence cleanup reported an error.
    fn finalize_abort_resources(&self, context: &Arc<TransactionContext>) {
        let released = context.mark_resources_released();
        if released {
            rollback_context_timestamp(&self.version_manager, context);
            if context.txn_type == TransactionType::Write {
                if context.has_pessimistic_lock() {
                    self.write_exclusion_owner.store(0, Ordering::SeqCst);
                }
                self.checkpoint_gate.release_write();
            }
        }
        // SSI: unregister read locks on abort.
        self.certifier.unregister_reads(context.id);
        self.active_transactions.remove(&context.id);
        if let Err(error) = context.clear_undo_logs() {
            log::warn!(
                "undo log cleanup failed after abort for txn={:?} write_ts={}: {}",
                context.id,
                context.timestamp(),
                error
            );
        }
        if let Err(error) = context.transition_to(TransactionState::Aborted) {
            log::error!(
                "state transition to Aborted failed for txn={:?} state={:?}: {}",
                context.id,
                context.state(),
                error
            );
        } else {
            log::info!(
                "transaction aborted: txn={:?} owner={:?}",
                context.id,
                context.owner()
            );
        }
        if released {
            self.emit_rollback_event(TransactionEvent::Aborted {
                txn_id: context.id,
                write_timestamp: context.timestamp(),
            });
        }
    }

    /// Called when the commit sink's abort fails after retries.
    /// Logs the failure, releases resources, and removes the context.
    fn finalize_resources_after_sink_failure(&self, context: &Arc<TransactionContext>) {
        self.finalize_abort_resources(context);
        self.stats.increment_cleanup_failure();
    }
}
