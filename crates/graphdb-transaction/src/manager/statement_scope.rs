//! Transaction manager behavior: statement snapshots pinned in the global snapshot tracker

use std::sync::Arc;

use super::TransactionManager;
use crate::context::TransactionContext;
use crate::error::TransactionError;
use crate::types::*;
use graphdb_core::types::Timestamp;
impl TransactionManager {
    /// Begin a statement in a transaction and refresh a READ COMMITTED
    /// snapshot. Repeatable-read transactions keep their original snapshot.
    ///
    /// Single entry point (no statement-kind flag): the statement kind is not
    /// knowable here — the query is planned after the statement scope opens —
    /// so read-only enforcement lives downstream instead of an intent
    /// parameter: the query layer rejects write operators in read-only scopes
    /// before execution (`reject_writes_outside_transaction_scope`),
    /// `TransactionContext::record_mutation` rejects journal writes on
    /// read-only transactions if they ever reach the journal, and commit
    /// certification rejects them at commit time.
    pub fn begin_statement(
        &self,
        txn_id: TransactionId,
    ) -> Result<(Arc<TransactionContext>, std::time::Instant), TransactionError> {
        let context = self.get_context(txn_id)?;
        if context.isolation_level == IsolationLevel::ReadCommitted {
            let committed = self.version_manager.read_timestamp();
            let snapshot = committed.max(context.timestamp());
            context.set_refreshed_read_ts(snapshot);
            let start = context.begin_statement()?;
            self.pin_statement_snapshot(&context, snapshot)?;
            self.stats.begin_statement();
            Ok((context, start))
        } else {
            let start = context.begin_statement()?;
            self.pin_statement_snapshot(&context, context.timestamp())?;
            self.stats.begin_statement();
            Ok((context, start))
        }
    }

    /// Refresh a transaction's statement snapshot without opening a
    /// materialized statement scope. This is used by lazy result streams.
    ///
    /// Same single-entry contract as `begin_statement`: no statement-kind
    /// flag, read-only enforcement lives in the query layer, the mutation
    /// recorder, and commit certification.
    pub fn refresh_statement_snapshot(
        &self,
        txn_id: TransactionId,
    ) -> Result<Arc<TransactionContext>, TransactionError> {
        let context = self.get_context(txn_id)?;
        context.can_execute()?;
        context.check_timeouts()?;
        if context.isolation_level == IsolationLevel::ReadCommitted {
            let committed = self.version_manager.read_timestamp();
            let snapshot = committed.max(context.timestamp());
            context.set_refreshed_read_ts(snapshot);
            self.pin_statement_snapshot(&context, snapshot)?;
        } else {
            self.pin_statement_snapshot(&context, context.timestamp())?;
        }
        Ok(context)
    }

    /// Finish a statement and record timeout failures as rollback-only.
    pub fn finish_statement(
        &self,
        context: &TransactionContext,
        statement_start: std::time::Instant,
    ) -> Result<(), TransactionError> {
        let result = context.finish_statement(statement_start);
        self.release_statement_snapshot_pin(context);
        self.stats.end_statement();
        if let Err(err) = &result {
            use crate::error::TransactionErrorKind;
            if matches!(
                err.kind(),
                TransactionErrorKind::TransactionTimeout
                    | TransactionErrorKind::TransactionExpired
                    | TransactionErrorKind::CheckpointTimeout
            ) {
                self.stats.record_timeout();
                if let Some(observable) = &self.observable {
                    observable.record_txn_timeout();
                }
            }
        }
        result
    }

    /// Pin a statement snapshot in the global snapshot tracker.
    ///
    /// Registers `snapshot` and installs it as the context pin, releasing
    /// any previous pin first so refresh-replace never leaks. GC derives
    /// its cutoff from the tracker, so a pinned statement snapshot cannot
    /// be reclaimed while the statement runs.
    fn pin_statement_snapshot(
        &self,
        context: &Arc<TransactionContext>,
        snapshot: Timestamp,
    ) -> Result<(), TransactionError> {
        if let Err(error) = self
            .version_manager
            .snapshot_tracker()
            .add_snapshot(snapshot)
        {
            return Err(TransactionError::internal(format!(
                "Failed to pin statement snapshot {}: {}",
                snapshot, error
            )));
        }
        if let Some(previous) = context.set_statement_snapshot_pin(snapshot) {
            let _ = self
                .version_manager
                .snapshot_tracker()
                .release_snapshot(previous);
        }
        Ok(())
    }

    /// Release the statement snapshot pin held by `context`, if any.
    ///
    /// Called on every terminal path (statement finish, commit, abort,
    /// recovery re-drive) so pins never outlive their statement. Releasing
    /// a missing entry is ignored: the pin may already have been replaced.
    pub(crate) fn release_statement_snapshot_pin(&self, context: &TransactionContext) {
        if let Some(pinned) = context.take_statement_snapshot_pin() {
            if let Err(error) = self
                .version_manager
                .snapshot_tracker()
                .release_snapshot(pinned)
            {
                log::debug!(
                    "Statement snapshot {} release skipped for txn={:?}: {}",
                    pinned,
                    context.id,
                    error
                );
            }
        }
    }
}
