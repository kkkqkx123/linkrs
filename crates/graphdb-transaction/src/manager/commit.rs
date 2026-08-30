//! Transaction commit protocol

use std::sync::atomic::Ordering;

use super::TransactionManager;
use crate::context::TransactionContext;
use crate::error::TransactionError;
use crate::participant::TransactionCommitDescriptor;
use crate::types::*;
use graphdb_core::types::CommitLsn;

impl TransactionManager {
    /// Commit transaction
    ///
    /// Follows atomic commit protocol:
    /// 1. Check state and timeout (transaction still active)
    /// 2. Transition to Committing (marks in-progress, prevents concurrent operations)
    /// 3. Persist through the configured storage commit sink with exponential backoff retries
    /// 4. Finalize commit and clear undo logs
    /// 5. Remove from active_transactions
    /// 6. Update stats
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

            if ctx.is_rollback_only() {
                return Err(TransactionError::invalid_state_for_commit(ctx.state()));
            }

            if ctx.check_timeouts().is_err() {
                self.stats.record_timeout();
                if let Err(error) = self.abort_transaction_internal(&ctx) {
                    log::error!(
                        "Abort-after-timeout failed for transaction {}: {}",
                        txn_id,
                        error
                    );
                    self.stats.increment_cleanup_failure();
                }
                return Err(TransactionError::transaction_timeout());
            }

            ctx
        };

        if context.txn_type != TransactionType::Checkpoint {
            if let Err(conflict) = self.check_write_set_conflict(txn_id) {
                if let Err(error) = self.abort_transaction_internal(&context) {
                    log::error!(
                        "Abort-after-conflict failed for transaction {}: {}",
                        txn_id,
                        error
                    );
                    self.stats.increment_cleanup_failure();
                }
                return Err(conflict);
            }
        }

        context.transition_to(TransactionState::Committing)?;
        let descriptor = TransactionCommitDescriptor {
            transaction_id: context.id,
            write_timestamp: context.timestamp(),
            durability: context.durability,
            write_set: context.get_write_set(),
        };
        let mut commit_lsn = CommitLsn::ZERO;

        if context.txn_type != TransactionType::Checkpoint {
            if let Some(ref commit_sink) = self.commit_sink {
                let max_retries = self.config.commit_retry_attempts;
                let mut last_error = None;

                for attempt in 0..=max_retries {
                    match commit_sink.commit_transaction_with_descriptor(&descriptor) {
                        Ok(lsn) => {
                            commit_lsn = lsn;
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
                    if let Err(abort_error) = self.abort_transaction_internal(&context) {
                        log::error!(
                            "Commit failure for transaction {} also failed abort finalization: {}",
                            txn_id,
                            abort_error
                        );
                        self.stats.increment_cleanup_failure();
                    }
                    return Err(TransactionError::commit_failed(format!(
                        "Failed to persist transaction {} after {} retries: {}",
                        txn_id, max_retries, err
                    )));
                }
            }
        }

        match context.txn_type {
            TransactionType::ReadOnly => self
                .version_manager
                .release_read_timestamp_at(context.start_timestamp),
            TransactionType::Write => {
                self.version_manager
                    .commit_write_timestamp(context.timestamp());
                if !descriptor.write_set.is_empty() {
                    // Publish the committed write set into the conflict indices.
                    // The certifier re-runs the final review under the
                    // certification shard lock to close the cross-shard
                    // certification race window.
                    if let Err(conflict) = self.certifier.publish(
                        context.id,
                        descriptor.write_timestamp,
                        context.start_timestamp,
                        &descriptor.write_set,
                        &self.active_transactions,
                        &self.stats,
                    ) {
                        if let Err(abort_error) = self.abort_transaction_internal(&context) {
                            log::error!(
                                "Final-review abort failed for txn={:?}: {}",
                                context.id,
                                abort_error
                            );
                            self.stats.increment_cleanup_failure();
                        }
                        return Err(conflict);
                    }
                }
                let safe_ts = self.version_manager.get_safe_gc_timestamp();
                self.prune_committed_write_sets(safe_ts);
                if context.has_pessimistic_lock() {
                    self.write_exclusion_owner.store(0, Ordering::SeqCst);
                }
                self.checkpoint_gate.release_write();
            }
            TransactionType::Checkpoint => {}
        }
        context.mark_commit_published(commit_lsn);

        if context.txn_type != TransactionType::Checkpoint {
            if let Some(ref commit_sink) = self.commit_sink {
                let max_retries = self.config.commit_retry_attempts;
                let mut last_error = None;
                for attempt in 0..=max_retries {
                    match commit_sink.finalize_commit(&descriptor, commit_lsn) {
                        Ok(()) => {
                            last_error = None;
                            break;
                        }
                        Err(error) => {
                            last_error = Some(error);
                            if attempt < max_retries {
                                std::thread::sleep(Self::backoff_delay(attempt));
                            }
                        }
                    }
                }
                if let Some(error) = last_error {
                    log::error!(
                        "Commit {} is durable but finalization failed after {} retries: {}",
                        txn_id,
                        max_retries,
                        error
                    );
                    self.recovery
                        .record(txn_id, context.timestamp(), commit_lsn);
                    self.active_transactions.remove(&txn_id);
                    let _ = context.transition_to(TransactionState::Aborting);
                    let _ = context.transition_to(TransactionState::Aborted);
                    self.emit_commit_event(TransactionEvent::CommitDurableButUnfinalized {
                        txn_id,
                        write_timestamp: context.timestamp(),
                        commit_lsn,
                    });
                    return Err(TransactionError::commit_failed(format!(
                        "Commit {} is durable but finalization failed: {}",
                        txn_id, error
                    )));
                }
            }
        }

        if let Err(error) = context.clear_undo_logs() {
            log::error!(
                "Commit {} is durable but undo-log cleanup failed: {}",
                txn_id,
                error
            );
            self.recovery
                .record(txn_id, context.timestamp(), commit_lsn);
            self.active_transactions.remove(&txn_id);
            let _ = context.transition_to(TransactionState::Aborting);
            let _ = context.transition_to(TransactionState::Aborted);
            self.emit_commit_event(TransactionEvent::CommitDurableButUnfinalized {
                txn_id,
                write_timestamp: context.timestamp(),
                commit_lsn,
            });
            return Err(TransactionError::commit_failed(format!(
                "Commit {} is durable but undo-log cleanup failed: {}",
                txn_id, error
            )));
        }

        self.active_transactions.remove(&txn_id);
        self.emit_commit_event(TransactionEvent::Committed {
            txn_id,
            write_timestamp: context.timestamp(),
            write_set: Box::new(descriptor.write_set),
            schema_catalog_version: context.schema_catalog_version(),
        });

        log::info!(
            "transaction committed: txn={:?} commit_lsn={:?} write_ts={}",
            txn_id,
            commit_lsn,
            context.timestamp()
        );

        Ok(())
    }

    /// Finalize a transaction through the single lifecycle dispatch point.
    pub fn finalize_transaction(
        &self,
        txn_id: TransactionId,
        outcome: TransactionOutcome,
    ) -> Result<(), TransactionError> {
        match outcome {
            TransactionOutcome::Commit => self.commit_transaction(txn_id),
            TransactionOutcome::Abort => self.abort_transaction(txn_id),
        }
    }

    /// Run one write operation in a transaction owned by this call.
    pub fn auto_commit<F, T, E>(&self, operation: F) -> Result<T, TransactionError>
    where
        F: FnOnce(&TransactionContext) -> Result<T, E>,
        E: Into<TransactionError>,
    {
        let txn_id = self.begin_insert_transaction(TransactionOptions::default())?;
        let context = self.get_context(txn_id)?;
        match operation(&context) {
            Ok(result) => {
                self.commit_transaction(txn_id)?;
                Ok(result)
            }
            Err(error) => {
                if let Err(abort_error) = self.abort_transaction(txn_id) {
                    log::error!(
                        "Auto-commit rollback failed for transaction {}: {}",
                        txn_id,
                        abort_error
                    );
                }
                Err(error.into())
            }
        }
    }

    /// Abort a transaction on behalf of its owner.
    pub fn kill_transaction(
        &self,
        txn_id: TransactionId,
        owner: Option<&str>,
    ) -> Result<(), TransactionError> {
        self.check_transaction_owner(txn_id, owner)?;
        self.abort_transaction(txn_id)
    }

    pub fn commit_transaction_as_owner(
        &self,
        txn_id: TransactionId,
        owner: Option<&str>,
    ) -> Result<(), TransactionError> {
        self.check_transaction_owner(txn_id, owner)?;
        self.commit_transaction(txn_id)
    }

    pub fn abort_transaction_as_owner(
        &self,
        txn_id: TransactionId,
        owner: Option<&str>,
    ) -> Result<(), TransactionError> {
        self.check_transaction_owner(txn_id, owner)?;
        self.abort_transaction(txn_id)
    }
}
