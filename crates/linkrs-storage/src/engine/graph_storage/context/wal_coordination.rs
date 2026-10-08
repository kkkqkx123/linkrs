use linkrs_core::types::Timestamp;

use super::GraphStorageContext;

impl GraphStorageContext {
    pub(crate) fn append_wal_redo<T: serde::Serialize>(
        &self,
        op_type: linkrs_core::wal::types::WalOpType,
        timestamp: Timestamp,
        redo: &T,
    ) -> linkrs_core::StorageResult<linkrs_transaction::wal::TransactionWalEntry> {
        let payload = postcard::to_allocvec(redo).map_err(|error| {
            linkrs_core::StorageError::serialize_error(format!(
                "Failed to serialize WAL redo: {}",
                error
            ))
        })?;
        if let Some(transaction_id) = self
            .operation_context
            .as_ref()
            .and_then(|operation| operation.transaction_id)
        {
            let entry = linkrs_transaction::wal::TransactionWalEntry {
                op_type,
                timestamp,
                payload,
                transaction_id: Some(transaction_id),
                mutation_sequence: None,
            };
            self.persistent
                .staged_wal
                .entry(transaction_id)
                .or_default()
                .push(entry.clone());
            return Ok(entry);
        }
        if let Some(persistence) = self.persistent.persistence.as_ref() {
            let wal_manager = {
                let coordinator = persistence.read();
                coordinator.wal_manager()
            };
            if let Some(wal) = wal_manager {
                wal.read().append_redo(op_type, timestamp, redo)?;
                return Ok(linkrs_transaction::wal::TransactionWalEntry {
                    op_type,
                    timestamp,
                    payload,
                    transaction_id: None,
                    mutation_sequence: None,
                });
            }
        }

        Ok(linkrs_transaction::wal::TransactionWalEntry {
            op_type,
            timestamp,
            payload,
            transaction_id: None,
            mutation_sequence: None,
        })
    }

    pub(crate) fn commit_staged_writes(
        &self,
        transaction_id: linkrs_core::types::TransactionId,
        intents: &[linkrs_core::wal::OutboxIntent],
    ) -> linkrs_core::StorageResult<linkrs_core::types::CommitLsn> {
        // Write-scope commit hook point: vertex scopes settle in the writer
        // ahead of the timestamp commit; this WAL entry is the durability
        // point for already scoped bindings.
        self.commit_staged_writes_with_durability(
            transaction_id,
            intents,
            linkrs_core::types::DurabilityLevel::Sync,
        )
    }

    pub(crate) fn commit_staged_writes_with_durability(
        &self,
        transaction_id: linkrs_core::types::TransactionId,
        intents: &[linkrs_core::wal::OutboxIntent],
        durability: linkrs_core::types::DurabilityLevel,
    ) -> linkrs_core::StorageResult<linkrs_core::types::CommitLsn> {
        // Explicit-transaction commit point: staged vertex rows are applied
        // before the WAL append, and the WAL durability plus barrier is the
        // publication. A durability failure undoes every applied mutation
        // (inserts, updates, deletes) so no committed-timestamp residue
        // leaks to later readers. Auto-commit statements certify in
        // `finalize_operation` after this call, so their apply is deferred
        // there.
        let defer_apply = self
            .operation_context
            .as_ref()
            .is_some_and(|operation| operation.auto_commit);
        let applied = if defer_apply {
            Vec::new()
        } else {
            self.apply_txn_staging(transaction_id)?
        };
        let entries = self
            .persistent
            .staged_wal
            .get(&transaction_id)
            .map(|entries| entries.clone())
            .unwrap_or_default();
        let commit_lsn = if let Some(persistence) = self.persistent.persistence.as_ref() {
            let wal_manager = persistence.read().wal_manager().ok_or_else(|| {
                linkrs_core::StorageError::wal_error("WAL manager is not initialized".to_string())
            })?;
            let result = match wal_manager.read().append_transaction_with_durability(
                transaction_id,
                entries,
                intents,
                durability,
            ) {
                Ok(result) => result,
                Err(error) => {
                    self.undo_applied_staging(&applied);
                    return Err(error);
                }
            };
            result
        } else {
            linkrs_core::types::CommitLsn::ZERO
        };
        self.persistent
            .index_data_manager
            .read()
            .advance_barriers(commit_lsn);
        self.persistent.staged_wal.remove(&transaction_id);
        Ok(commit_lsn)
    }

    /// Flush the group's accumulated WAL redo with `DurabilityLevel::None`
    /// (no fsync): called once at the group commit point after the staged
    /// vertex rows are applied. Barriers are deferred to the group sync in
    /// `finalize_group`.
    pub(crate) fn commit_staged_writes_grouped(
        &self,
        transaction_id: linkrs_core::types::TransactionId,
        intents: &[linkrs_core::wal::OutboxIntent],
    ) -> linkrs_core::StorageResult<linkrs_core::types::CommitLsn> {
        let entries = self
            .persistent
            .staged_wal
            .get(&transaction_id)
            .map(|entries| entries.clone())
            .unwrap_or_default();
        let commit_lsn = if let Some(persistence) = self.persistent.persistence.as_ref() {
            let guard = persistence.read();
            let wal_manager = guard.wal_manager().ok_or_else(|| {
                linkrs_core::StorageError::wal_error("WAL manager is not initialized".to_string())
            })?;
            let result = wal_manager.read().append_transaction_with_durability(
                transaction_id,
                entries,
                intents,
                linkrs_core::types::DurabilityLevel::None,
            )?;
            result
        } else {
            linkrs_core::types::CommitLsn::ZERO
        };
        // Deliberately no advance_barriers: deferred to finalize_group.
        self.persistent.staged_wal.remove(&transaction_id);
        Ok(commit_lsn)
    }

    pub(crate) fn abort_staged_writes(&self, transaction_id: linkrs_core::types::TransactionId) {
        // Abort point for a whole statement transaction: drop the staged
        // WAL redo and discard the staging buffer, releasing every
        // reservation it held (nothing was applied for staged-only rows).
        self.persistent.staged_wal.remove(&transaction_id);
        self.discard_txn_staging_for(transaction_id);
    }

    /// Number of staged-WAL entries held for in-flight transactions.
    pub(crate) fn staged_wal_len(&self) -> usize {
        self.persistent.staged_wal.len()
    }

    /// Staged-WAL entries accumulated for one transaction.
    pub(crate) fn staged_wal_len_for(
        &self,
        transaction_id: linkrs_core::types::TransactionId,
    ) -> usize {
        self.persistent
            .staged_wal
            .get(&transaction_id)
            .map(|entries| entries.len())
            .unwrap_or(0)
    }

    /// Drop one transaction's staged WAL redo without touching the buffer.
    pub(crate) fn drop_staged_wal_for(&self, transaction_id: linkrs_core::types::TransactionId) {
        self.persistent.staged_wal.remove(&transaction_id);
    }

    /// Whether this context is bound inside a group-mode
    /// [`AutoCommitBatchWindow`].
    pub(crate) fn is_group_bound(&self) -> bool {
        self.auto_commit_window
            .as_ref()
            .is_some_and(|w| w.is_grouped())
    }

    pub(crate) fn defer_edge_insert(
        &self,
        edge: linkrs_core::wal::redo::InsertEdgeRedo,
        ts: Timestamp,
    ) {
        self.runtime.deferred_wal_ops.push_edge(edge, ts);
    }

    pub(crate) fn defer_edge_delete(
        &self,
        delete: linkrs_core::wal::redo::DeleteEdgeRedo,
        ts: Timestamp,
    ) {
        self.runtime.deferred_wal_ops.push_delete(delete, ts);
    }

    pub(crate) fn take_deferred_edge_inserts(
        &self,
    ) -> Vec<(linkrs_core::wal::redo::InsertEdgeRedo, Timestamp)> {
        self.runtime.deferred_wal_ops.drain_edges()
    }

    pub(crate) fn take_deferred_edge_deletes(
        &self,
    ) -> Vec<(linkrs_core::wal::redo::DeleteEdgeRedo, Timestamp)> {
        self.runtime.deferred_wal_ops.drain_deletes()
    }
}
