use graphdb_core::{StorageError, StorageResult};

use super::GraphStorage;

impl crate::StorageCommitOps for GraphStorage {
    fn commit_staged_writes(
        &self,
        transaction_id: graphdb_core::types::TransactionId,
        intents: &[graphdb_core::wal::OutboxIntent],
    ) -> StorageResult<graphdb_core::types::CommitLsn> {
        self.ctx.commit_staged_writes(transaction_id, intents)
    }

    fn abort_staged_writes(
        &self,
        transaction_id: graphdb_core::types::TransactionId,
    ) -> StorageResult<()> {
        self.ctx.abort_staged_writes(transaction_id);
        Ok(())
    }

    fn commit_staged_writes_with_durability(
        &self,
        transaction_id: graphdb_core::types::TransactionId,
        intents: &[graphdb_core::wal::OutboxIntent],
        durability: graphdb_core::types::DurabilityLevel,
    ) -> StorageResult<graphdb_core::types::CommitLsn> {
        self.ctx
            .commit_staged_writes_with_durability(transaction_id, intents, durability)
    }

    fn recover_outbox_projection(
        &self,
        sync_manager: &graphdb_sync::SyncManager,
    ) -> StorageResult<usize> {
        use graphdb_transaction::wal::{collect_committed_transactions, LocalWalParser, WalParser};
        let Some(paths) = self.ctx.storage_paths() else {
            return Ok(0);
        };
        if !paths.wal_dir().exists() {
            return Ok(0);
        }

        let snapshot_lsn = sync_manager.outbox_materialized_lsn().map_err(|error| {
            StorageError::db_error(format!(
                "Failed to read outbox materialization frontier: {}",
                error
            ))
        })?;

        let mut parser = LocalWalParser::new();
        parser
            .open(&paths.wal_dir().to_string_lossy())
            .map_err(|error| {
                StorageError::wal_error(format!(
                    "Failed to parse WAL for outbox recovery: {}",
                    error
                ))
            })?;
        let transactions =
            collect_committed_transactions(&parser.parse_all_entries()).map_err(|error| {
                StorageError::wal_error(format!(
                    "Failed to validate WAL for outbox recovery: {}",
                    error
                ))
            })?;

        let mut recovered = 0usize;
        for transaction in transactions {
            if transaction.intents.is_empty() {
                continue;
            }
            if let Some(snapshot_lsn) = snapshot_lsn {
                if transaction.commit_lsn <= snapshot_lsn {
                    continue;
                }
            }
            sync_manager
                .materialize_committed_transaction(
                    transaction.transaction_id,
                    transaction.commit_lsn,
                    &transaction.intents,
                )
                .map_err(|error| {
                    StorageError::db_error(format!(
                        "Failed to recover outbox transaction {}: {}",
                        transaction.transaction_id, error
                    ))
                })?;
            recovered = recovered.saturating_add(transaction.intents.len());
        }

        log::info!(
            "Outbox projection recovery complete: {} intents replayed (snapshot_lsn={:?})",
            recovered,
            snapshot_lsn
        );

        Ok(recovered)
    }
}
