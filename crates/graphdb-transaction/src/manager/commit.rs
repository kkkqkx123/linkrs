//! Transaction commit protocol

use std::sync::atomic::Ordering;

use super::TransactionManager;
use crate::context::TransactionContext;
use crate::error::TransactionError;
use crate::types::*;
use graphdb_core::types::CommitLsn;

impl TransactionManager {
    /// Commit transaction
    ///
    /// Follows atomic commit protocol:
    /// 1. Check state and timeout (transaction still active)
    /// 2. Transition to Committing (marks in-progress, prevents concurrent operations)
    /// 3. Certify the write set (pre-check; conflicts abort before any I/O)
    /// 4. Persist through the configured storage commit sink with exponential backoff retries
    /// 5. Publish the write set into the certifier
    /// 6. Finalize through the commit sink (storage visibility point)
    /// 7. Allocate a commit timestamp and advance the read frontier, so read
    ///    visibility is ordered by commit time and never precedes durability
    /// 8. Clear undo logs, transition to Committed, remove from active_transactions
    /// 9. Update stats
    ///
    /// If storage finalization fails after the WAL is durable, the transaction
    /// stays `Committing` in the active table, the finalization is queued for
    /// recovery, and an error is returned: the read frontier is NOT advanced,
    /// so new readers cannot observe the unfinalized writes. Use
    /// `recover_pending_finalization` to re-drive finalization.
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

        if context.txn_type.is_user_transaction() {
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

        // Distributed transactions with Recovery or Dummy markers must not flow
        // through the normal user commit path.  Recovery only finalizes
        // through recover_unfinalized_commits and Dummy never reaches WAL.
        if context.get_type() == TransactionType::Recovery
            || context.get_type() == TransactionType::Dummy
        {
            return Err(TransactionError::invalid_state_for_commit(context.state()));
        }

        context.transition_to(TransactionState::Committing)?;
        let mut descriptor = context.build_commit_descriptor();
        let mut commit_lsn = CommitLsn::ZERO;

        if context.txn_type.is_user_transaction() {
            if !self.config.in_memory {
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
            } else {
                // In-memory mode: skip WAL durability, assign synthetic LSN
                commit_lsn = CommitLsn::new(context.timestamp());
                // Clear derived WAL cache without I/O (journal stays intact
                // as the source of truth for the in-memory commit).
                context.clear_derived_wal();
            }
        }

        // Certification publish runs before storage finalization so a commit
        // that fails finalization still defends its write set against later
        // committers (conservative: later conflicts abort rather than risk
        // lost updates). Visibility itself is only published afterwards via
        // the commit timestamp (see below).
        let mut needs_commit_ts = false;
        match context.txn_type {
            TransactionType::ReadOnly => self
                .version_manager
                .release_read_timestamp_at(context.start_timestamp),
            TransactionType::Write => {
                if !descriptor.write_set.is_empty() {
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
                // Release the gate lease before storage finalization (shared
                // exactly-once helper with the abort path): checkpoints drain
                // on pre-commit writes only and must not wait for finalize.
                self.release_write_lease(&context);
                needs_commit_ts = true;
            }
            TransactionType::Checkpoint | TransactionType::Recovery | TransactionType::Dummy => {}
        }
        context.mark_commit_published(commit_lsn);

        // Storage finalization is the visibility gate: only finalized commits
        // receive a commit timestamp. A finalization failure therefore keeps
        // the transaction `Committing` in the active table (visible to admins,
        // invisible to readers), queues the finalization for recovery, and
        // reports an error instead of success.
        if context.txn_type.is_user_transaction() && !self.config.in_memory {
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
                    self.recovery.record(&descriptor, 0, commit_lsn);
                    self.emit_commit_event(TransactionEvent::CommitDurableButUnfinalized {
                        txn_id,
                        write_timestamp: context.timestamp(),
                        commit_lsn,
                    });
                    // Retire the start slot without publishing visibility: the
                    // read frontier is not allowed to cross this commit, but it
                    // must not be pinned behind it either.
                    self.version_manager
                        .abort_write_timestamp(context.timestamp());
                    self.certifier.unregister_reads(context.id);
                    return Err(TransactionError::commit_failed(format!(
                        "Transaction {} is durable but unfinalized after {} retries: {}; \
                         re-drive with recover_pending_finalization",
                        txn_id, max_retries, error
                    )));
                }
            }
        }

        // Allocate the commit timestamp only after durability + finalization.
        // This is what advances the read frontier and stamps the journal, so
        // unfinalized writes can never become visible to new readers.
        let commit_ts = if needs_commit_ts {
            match self
                .version_manager
                .allocate_commit_timestamp(context.timestamp())
            {
                Ok(commit_ts) => {
                    context.set_commit_timestamp(commit_ts);
                    descriptor.commit_timestamp = commit_ts;
                    // Distinguish write vs commit timestamp in the journal so
                    // GC can tell committed history from still-pending history.
                    context.publish_commit_timestamp(commit_ts);
                    commit_ts
                }
                Err(error) => {
                    log::error!(
                        "Commit {} is durable and finalized but commit-timestamp allocation failed: {}",
                        txn_id,
                        error
                    );
                    self.recovery.record(&descriptor, 0, commit_lsn);
                    self.emit_commit_event(TransactionEvent::CommitDurableButUnfinalized {
                        txn_id,
                        write_timestamp: context.timestamp(),
                        commit_lsn,
                    });
                    return Err(TransactionError::commit_failed(format!(
                        "Failed to allocate commit timestamp for transaction {}: {}",
                        txn_id, error
                    )));
                }
            }
        } else {
            0
        };

        if let Err(error) = context.clear_undo_logs() {
            // The commit is already durable, finalized and visible at this
            // point, so only queue recovery and report success instead of
            // rewriting the state to Aborted.
            log::error!(
                "Commit {} is durable but undo-log cleanup failed: {}",
                txn_id,
                error
            );
            self.recovery.record(&descriptor, commit_ts, commit_lsn);
            self.active_transactions.remove(&txn_id);
            self.emit_commit_event(TransactionEvent::CommitDurableButUnfinalized {
                txn_id,
                write_timestamp: context.timestamp(),
                commit_lsn,
            });
            return Ok(());
        }

        context.transition_to(TransactionState::Committed)?;
        self.active_transactions.remove(&txn_id);
        self.emit_commit_event(TransactionEvent::Committed {
            txn_id,
            write_timestamp: context.timestamp(),
            commit_timestamp: commit_ts,
            write_set: Box::new(descriptor.write_set),
            schema_catalog_version: context.schema_catalog_version(),
        });

        log::info!(
            "transaction committed: txn={:?} commit_lsn={:?} write_ts={} commit_ts={}",
            txn_id,
            commit_lsn,
            context.timestamp(),
            commit_ts
        );

        if context.txn_type == TransactionType::Write {
            self.commits_since_checkpoint.fetch_add(1, Ordering::SeqCst);
        }

        if self.should_auto_checkpoint() && context.txn_type == TransactionType::Write {
            if let Some(ref commit_sink) = self.commit_sink {
                if let Err(e) = commit_sink.auto_checkpoint_if_needed() {
                    log::warn!(
                        "Auto-checkpoint after commit {} failed (non-fatal): {}",
                        txn_id,
                        e
                    );
                }
            }
        }

        Ok(())
    }

    /// Re-drive finalization for a durable-but-unfinalized transaction.
    ///
    /// Takes the queued pending record for `txn_id`, retries storage
    /// finalization, then allocates the commit timestamp (if not yet
    /// allocated), publishes visibility, transitions the transaction to
    /// `Committed`, and emits the commit event. The re-drive is exclusive:
    /// the pending record is taken before retrying and re-queued on failure,
    /// so concurrent recovery attempts cannot double-finalize.
    ///
    /// If the transaction context is already gone (e.g. after a restart),
    /// storage finalization is still retried best-effort from the stored
    /// descriptor and success is reported: there is no journal left to stamp,
    /// and page-level replay belongs to the storage engine.
    pub fn recover_pending_finalization(
        &self,
        txn_id: TransactionId,
    ) -> Result<(), TransactionError> {
        let pending = self
            .recovery
            .take_pending(txn_id)
            .ok_or_else(|| TransactionError::transaction_not_found(txn_id))?;

        let requeue = |manager: &Self| {
            manager.recovery.record(
                &pending.descriptor,
                pending.commit_timestamp,
                pending.commit_lsn,
            );
        };

        let context = match self.get_context(txn_id) {
            Ok(context) => Some(context),
            Err(_) => None,
        };
        let Some(context) = context else {
            // No transaction-level state left to complete; still give the
            // storage sink a chance to finish idempotently.
            if !self.config.in_memory {
                if let Some(ref commit_sink) = self.commit_sink {
                    let _ = commit_sink.finalize_commit(&pending.descriptor, pending.commit_lsn);
                }
            }
            return Ok(());
        };

        if context.state() != TransactionState::Committing || !context.commit_published() {
            requeue(self);
            return Err(TransactionError::invalid_state_for_commit(context.state()));
        }

        if !self.config.in_memory {
            if let Some(ref commit_sink) = self.commit_sink {
                let max_retries = self.config.commit_retry_attempts;
                let mut last_error = None;
                for attempt in 0..=max_retries {
                    match commit_sink.finalize_commit(&pending.descriptor, pending.commit_lsn) {
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
                    requeue(self);
                    return Err(TransactionError::commit_failed(format!(
                        "Recovery finalization failed for transaction {}: {}",
                        txn_id, error
                    )));
                }
            }
        }

        let mut descriptor = pending.descriptor.clone();
        let commit_ts = if pending.commit_timestamp == 0 && context.commit_timestamp() == 0 {
            match self
                .version_manager
                .allocate_commit_timestamp(context.timestamp())
            {
                Ok(commit_ts) => {
                    context.set_commit_timestamp(commit_ts);
                    context.publish_commit_timestamp(commit_ts);
                    commit_ts
                }
                Err(error) => {
                    requeue(self);
                    return Err(TransactionError::commit_failed(format!(
                        "Recovery timestamp allocation failed for transaction {}: {}",
                        txn_id, error
                    )));
                }
            }
        } else {
            let commit_ts = context.commit_timestamp().max(pending.commit_timestamp);
            if context.commit_timestamp() == 0 && commit_ts != 0 {
                context.set_commit_timestamp(commit_ts);
                context.publish_commit_timestamp(commit_ts);
            }
            commit_ts
        };
        descriptor.commit_timestamp = commit_ts;

        if let Err(error) = context.clear_undo_logs() {
            requeue(self);
            return Err(TransactionError::rollback_failed(format!(
                "Recovery undo-log cleanup failed for transaction {}: {}",
                txn_id, error
            )));
        }

        context.transition_to(TransactionState::Committed)?;
        self.active_transactions.remove(&txn_id);
        self.certifier.unregister_reads(txn_id);
        self.emit_commit_event(TransactionEvent::Committed {
            txn_id,
            write_timestamp: context.timestamp(),
            commit_timestamp: commit_ts,
            write_set: Box::new(descriptor.write_set),
            schema_catalog_version: context.schema_catalog_version(),
        });
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
