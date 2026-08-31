use super::SyncWrapper;
use crate::engine::graph_storage::AutoCommitBatchWindow;
use crate::StorageClient;
use graphdb_core::StorageResult;
use std::sync::Arc;

impl<S: StorageClient + crate::AutoCommitBatchOps> crate::AutoCommitBatchOps for SyncWrapper<S> {
    fn begin_auto_commit_batch(&self) -> StorageResult<Arc<AutoCommitBatchWindow>> {
        self.inner.begin_auto_commit_batch()
    }

    fn bind_auto_commit_statement(
        &self,
        window: &Arc<AutoCommitBatchWindow>,
    ) -> StorageResult<Self> {
        let inner = self.inner.bind_auto_commit_statement(window)?;
        Ok(SyncWrapper {
            inner,
            sync_manager: self.sync_manager.clone(),
            enabled: self.enabled,
            auto_commit_owner: self.auto_commit_owner,
        })
    }

    fn finalize_auto_commit_batch(&self, window: &AutoCommitBatchWindow) -> StorageResult<()> {
        self.inner.finalize_auto_commit_batch(window)
    }
}

impl<S: StorageClient + crate::AutoCommitGroupOps> crate::AutoCommitGroupOps for SyncWrapper<S> {
    fn begin_auto_commit_group(&self) -> StorageResult<Arc<AutoCommitBatchWindow>> {
        self.inner.begin_auto_commit_group()
    }

    fn finalize_auto_commit_group(&self, window: &AutoCommitBatchWindow) -> StorageResult<()> {
        self.inner.finalize_auto_commit_group(window)
    }
}

impl<S: StorageClient + graphdb_transaction::UndoTarget + 'static>
    graphdb_transaction::TransactionCommitSink for SyncWrapper<S>
{
    fn commit_transaction(
        &self,
        transaction_id: graphdb_core::types::TransactionId,
    ) -> Result<graphdb_core::types::CommitLsn, String> {
        self.commit_transaction_fact(transaction_id)
            .map_err(|error| error.to_string())
    }

    fn commit_transaction_with_descriptor(
        &self,
        descriptor: &graphdb_transaction::TransactionCommitDescriptor,
    ) -> Result<graphdb_core::types::CommitLsn, String> {
        self.commit_transaction_fact_with_durability(
            descriptor.transaction_id,
            descriptor.durability,
        )
        .map_err(|error| error.to_string())
    }

    fn finalize_commit(
        &self,
        descriptor: &graphdb_transaction::TransactionCommitDescriptor,
        commit_lsn: graphdb_core::types::CommitLsn,
    ) -> Result<(), String> {
        self.finalize_commit_fact(descriptor.transaction_id, commit_lsn)
            .map_err(|error| error.to_string())
    }

    fn abort_transaction(
        &self,
        transaction_id: graphdb_core::types::TransactionId,
    ) -> Result<(), String> {
        self.abort_transaction_fact(transaction_id)
            .map_err(|error| error.to_string())
    }

    fn abort_transaction_with_descriptor(
        &self,
        descriptor: &graphdb_transaction::TransactionAbortDescriptor,
    ) -> Result<(), String> {
        descriptor
            .context
            .execute_undo_logs(&self.inner)
            .map_err(|error| error.to_string())?;
        descriptor
            .context
            .clear_undo_logs()
            .map_err(|error| error.to_string())?;
        self.abort_transaction_fact(descriptor.transaction_id)
            .map_err(|error| error.to_string())
    }
}
