//! Transaction abort protocol

use std::sync::Arc;

use super::{rollback_context_timestamp, TransactionManager};
use crate::context::TransactionContext;
use crate::error::TransactionError;
use crate::participant::TransactionAbortDescriptor;
use crate::rollback::UndoLogRollback;
use crate::types::*;
use crate::undo_log::UndoTarget;

impl TransactionManager {
    /// Abort transaction.
    ///
    /// Canonical abort entry without an explicit undo target: when a commit
    /// sink is configured, storage undo is delegated to the sink abort path;
    /// without a sink there is no storage to apply undo entries to, so they
    /// are dropped with a warning. Callers that own a storage target and run
    /// without a sink must use `abort_transaction_with_undo`.
    /// Both entries share `fetch_abortable_context` and `abort_common`, so
    /// checks and finalization cannot diverge.
    pub fn abort_transaction(&self, txn_id: TransactionId) -> Result<(), TransactionError> {
        let context = self.fetch_abortable_context(txn_id)?;
        self.abort_without_target(&context)
    }

    /// Abort transaction with undo target (for rollback support).
    ///
    /// Same protocol as `abort_transaction`, plus one pre-step: without a
    /// commit sink the local undo log is executed against `target`. With a
    /// sink the sink owns storage undo (executing locally as well would apply
    /// every entry twice), so the local target is skipped and undo is
    /// delegated to the sink abort path.
    pub fn abort_transaction_with_undo<T: UndoTarget + ?Sized>(
        &self,
        txn_id: TransactionId,
        target: &mut T,
    ) -> Result<(), TransactionError> {
        let context = self.fetch_abortable_context(txn_id)?;
        // Capture the local undo execution generically: the closure owns the
        // `&mut T` borrow, so the shared body never needs a `?Sized`-to-`dyn`
        // cast. With a sink the body ignores it (sink owns storage undo).
        let mut run_local_undo = || -> Result<(), TransactionError> {
            let rollback = UndoLogRollback::new(&*context);
            rollback
                .execute_rollback(&mut *target, context.timestamp())
                .map_err(|error| TransactionError::rollback_failed(error.to_string()))?;
            rollback
                .clear_logs()
                .map_err(|error| TransactionError::rollback_failed(error.to_string()))
        };
        self.abort_common(
            &context,
            Some(&mut run_local_undo as &mut dyn FnMut() -> Result<(), TransactionError>),
        )
    }

    /// Fetch a context that is allowed to abort.
    ///
    /// Shared by every abort entry point so the checks cannot diverge:
    /// the transaction must exist, must be in an abortable state, and must
    /// not hold a WAL-durable commit (durable commits complete via
    /// `recover_pending_finalization`, never via rollback).
    fn fetch_abortable_context(
        &self,
        txn_id: TransactionId,
    ) -> Result<Arc<TransactionContext>, TransactionError> {
        let entry = self
            .active_transactions
            .get(&txn_id)
            .ok_or(TransactionError::transaction_not_found(txn_id))?;
        let context = entry.value().clone();
        drop(entry);

        if !context.state().can_abort() {
            return Err(TransactionError::invalid_state_for_abort(context.state()));
        }
        if context.commit_published() {
            return Err(TransactionError::commit_failed(format!(
                "Transaction {} is already durable and cannot be aborted; \
                 re-drive finalization with recover_pending_finalization",
                txn_id
            )));
        }
        Ok(context)
    }

    /// Single abort body shared by every abort entry point.
    ///
    /// `run_local_undo` carries the caller-owned undo execution (generic over
    /// the storage target, so no `?Sized`-to-`dyn` cast is needed). It is only
    /// invoked when no commit sink is configured; with a sink, storage undo
    /// is delegated to the sink abort path inside `execute_abort_internal`
    /// (running both would apply every entry twice). `None` means "no
    /// explicit target": undo entries are dropped with a warning instead of
    /// being silently skipped.
    fn abort_common(
        &self,
        context: &Arc<TransactionContext>,
        run_local_undo: Option<&mut dyn FnMut() -> Result<(), TransactionError>>,
    ) -> Result<(), TransactionError> {
        // Single enforcement point for the durable-commit invariant: once the
        // commit record is WAL-durable (`mark_commit_published`), the
        // transaction can only complete via `recover_pending_finalization`,
        // never via rollback — through ANY abort entry, public or internal.
        if context.commit_published() {
            return Err(TransactionError::commit_failed(format!(
                "Transaction {} is already durable and cannot be aborted; \
                 re-drive finalization with recover_pending_finalization",
                context.id
            )));
        }
        context.transition_to(TransactionState::Aborting)?;

        match run_local_undo {
            Some(run) if self.commit_sink.is_none() => {
                if let Err(error) = run() {
                    let _ = self.execute_abort_internal(context);
                    return Err(error);
                }
            }
            None if self.commit_sink.is_none() && context.undo_log_len() > 0 => {
                log::warn!(
                    "Aborting transaction {:?} without an undo target or commit sink; \
                     dropping {} undo entries without executing them",
                    context.id,
                    context.undo_log_len()
                );
            }
            // With a sink, storage undo is delegated to the sink abort path
            // inside `execute_abort_internal` in both cases.
            _ => {}
        }

        self.execute_abort_internal(context)
    }

    /// Abort body without an explicit undo target.
    ///
    /// Thin wrapper over `abort_common` kept for call sites that own no
    /// storage target (timeouts, conflicts, shutdown, cleaner).
    fn abort_without_target(
        &self,
        context: &Arc<TransactionContext>,
    ) -> Result<(), TransactionError> {
        self.abort_common(context, None)
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
        self.abort_without_target(context)
    }

    /// Execute abort steps (transition already done by caller).
    fn execute_abort_internal(
        &self,
        context: &Arc<TransactionContext>,
    ) -> Result<(), TransactionError> {
        let max_retries = self.config.abort_retry_attempts;

        if !self.config.in_memory && context.txn_type.is_user_transaction() {
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
        // Timestamp retirement is idempotent by slot-state check, so it runs
        // unconditionally: the commit path may have released the gate lease
        // already (finalize-failure path leaves the transaction `Committing`)
        // while the start slot is still `Pending` (commit-timestamp
        // allocation failure). The gate lease itself goes through the shared
        // exactly-once helper.
        rollback_context_timestamp(&self.version_manager, context);
        self.release_write_lease(context);
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
            // `Aborted` is terminal and the transition above is CAS-guarded,
            // so this event fires exactly once per transaction.
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
