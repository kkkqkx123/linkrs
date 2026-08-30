//! Storage Layer Synchronous Wrapper
//!
//! Decorator pattern implementation that wraps any StorageClient to automatically
//! synchronize storage operations with external index systems (fulltext, vector).

use crate::StorageClient;
use graphdb_core::StorageError;
use std::fmt::Debug;
use std::sync::Arc;

/// Decorator that wraps a StorageClient to provide automatic index synchronization.
#[derive(Clone, Debug)]
pub struct SyncWrapper<S: StorageClient + Debug> {
    inner: S,
    sync_manager: Option<Arc<graphdb_sync::SyncManager>>,
    enabled: bool,
    auto_commit_owner: bool,
}

impl<S: StorageClient> SyncWrapper<S> {
    /// Create a new wrapper without synchronization.
    pub fn new(storage: S) -> Self {
        Self {
            inner: storage,
            sync_manager: None,
            enabled: false,
            auto_commit_owner: false,
        }
    }

    /// Create a new wrapper with a SyncManager for index synchronization.
    pub fn with_sync_manager(storage: S, sync_manager: Arc<graphdb_sync::SyncManager>) -> Self {
        let frontier_manager = sync_manager.clone();
        storage.set_outbox_materialized_lsn_provider(Arc::new(move || {
            frontier_manager
                .outbox_materialized_lsn()
                .map_err(|error| StorageError::db_error(error.to_string()))
        }));
        Self {
            inner: storage,
            sync_manager: Some(sync_manager),
            enabled: true,
            auto_commit_owner: false,
        }
    }

    /// Enable or disable synchronization.
    pub fn enable_sync(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Check if synchronization is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get reference to the sync manager.
    pub fn get_sync_manager(&self) -> Option<Arc<graphdb_sync::SyncManager>> {
        self.sync_manager.clone()
    }

    /// Get reference to the inner storage client.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Get mutable reference to the inner storage client.
    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }
}

impl<S: StorageClient> SyncWrapper<S> {
    /// Get the current transaction ID from storage context.
    fn get_current_txn_id(&self) -> Option<graphdb_core::types::TransactionId> {
        self.inner
            .operation_context()
            .and_then(|ctx| ctx.transaction_id)
    }
    fn commit_transaction_fact(
        &self,
        transaction_id: graphdb_core::types::TransactionId,
    ) -> Result<graphdb_core::types::CommitLsn, StorageError> {
        self.commit_transaction_fact_with_durability(
            transaction_id,
            graphdb_core::types::DurabilityLevel::Sync,
        )
    }

    fn commit_transaction_fact_with_durability(
        &self,
        transaction_id: graphdb_core::types::TransactionId,
        durability: graphdb_core::types::DurabilityLevel,
    ) -> Result<graphdb_core::types::CommitLsn, StorageError> {
        let intents = match self.sync_manager.as_ref() {
            Some(manager) => manager
                .pending_transaction_intents(transaction_id)
                .map_err(|error| {
                    StorageError::db_error(format!(
                        "Failed to build transaction index intents: {}",
                        error
                    ))
                })?,
            None => Vec::new(),
        };
        let commit_lsn = match self.inner.commit_staged_writes_with_durability(
            transaction_id,
            &intents,
            durability,
        ) {
            Ok(commit_lsn) => commit_lsn,
            Err(error) => {
                // A failed durability fence must not leave redo or target
                // intents attached to an auto-commit transaction that the
                // caller is allowed to retry.
                let _ = self.inner.abort_staged_writes(transaction_id);
                if let Some(manager) = self.sync_manager.as_ref() {
                    let _ = manager.rollback_transaction_sync(transaction_id);
                }
                return Err(error);
            }
        };
        Ok(commit_lsn)
    }

    fn finalize_commit_fact(
        &self,
        transaction_id: graphdb_core::types::TransactionId,
        commit_lsn: graphdb_core::types::CommitLsn,
    ) -> Result<(), StorageError> {
        if let Some(manager) = self.sync_manager.as_ref() {
            let intents = manager
                .pending_transaction_intents(transaction_id)
                .map_err(|error| {
                    StorageError::db_error(format!(
                        "Failed to load committed transaction intents: {}",
                        error
                    ))
                })?;
            if let Err(error) =
                manager.materialize_committed_transaction(transaction_id, commit_lsn, &intents)
            {
                log::error!(
                    "Committed transaction {} at {} but outbox materialization is pending recovery: {}",
                    transaction_id,
                    commit_lsn,
                    error
                );
            }
            if let Err(error) = manager.rollback_transaction_sync(transaction_id) {
                log::warn!(
                    "Committed transaction {} at {} but staging cleanup failed: {}",
                    transaction_id,
                    commit_lsn,
                    error
                );
            }
            manager.clear_transaction_intents(transaction_id);
            if let Err(error) = manager.retry_outbox_sync() {
                log::debug!(
                    "Committed transaction {} at {}; target delivery will retry: {}",
                    transaction_id,
                    commit_lsn,
                    error
                );
            }
        }
        Ok(())
    }

    fn commit_auto_transaction(
        &self,
    ) -> Result<Option<graphdb_core::types::CommitLsn>, StorageError> {
        let Some(context) = self.inner.operation_context() else {
            return Ok(None);
        };
        if (!self.auto_commit_owner && !context.auto_commit) || context.read_only {
            return Ok(None);
        }
        let transaction_id = context.transaction_id.ok_or_else(|| {
            StorageError::db_error("Auto-commit write has no transaction ID".to_string())
        })?;
        let commit_lsn = self.commit_transaction_fact(transaction_id)?;
        // Mirror the explicit-transaction sink flow: after the WAL durability
        // fence, materialize the staged intents into the durable outbox and
        // release them. Skipping this step would pin the outbox frontier at
        // its pre-commit value forever, which in turn pins the checkpoint
        // safe-WAL boundary at 0 (WAL never truncated) and leaks the staged
        // intents in `pending_intents`.
        self.finalize_commit_fact(transaction_id, commit_lsn)?;
        Ok(Some(commit_lsn))
    }

    fn abort_transaction_fact(
        &self,
        transaction_id: graphdb_core::types::TransactionId,
    ) -> Result<(), StorageError> {
        self.inner.abort_staged_writes(transaction_id)?;
        if let Some(manager) = self.sync_manager.as_ref() {
            manager
                .rollback_transaction_sync(transaction_id)
                .map_err(|error| {
                    StorageError::db_error(format!(
                        "Failed to discard transaction index intents: {}",
                        error
                    ))
                })?;
        }
        Ok(())
    }

    fn stage_index_create(
        &self,
        index: &graphdb_core::types::Index,
        index_type: &str,
    ) -> Result<(), StorageError> {
        if !self.enabled {
            return Ok(());
        }
        let Some(sync_manager) = self.get_sync_manager() else {
            return Ok(());
        };
        let transaction_id = self.get_current_txn_id().ok_or_else(|| {
            StorageError::db_error(
                "Synchronized schema changes require an operation transaction context".to_string(),
            )
        })?;
        let fields = index
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.value_type.clone()))
            .collect::<Vec<_>>();
        sync_manager
            .on_index_create(
                transaction_id,
                graphdb_sync::manager::IndexCreateRequest {
                    space_id: index.space_id,
                    index_name: index.name.clone(),
                    schema_name: index.schema_name.clone(),
                    index_type: index_type.to_string(),
                    fields,
                    properties: index.properties.clone(),
                },
            )
            .map_err(|error| {
                StorageError::db_error(format!("Failed to stage index creation intent: {error}"))
            })
    }

    fn validate_schema_sync_context(&self) -> Result<(), StorageError> {
        if self.enabled && self.get_sync_manager().is_some() && self.get_current_txn_id().is_none()
        {
            return Err(StorageError::db_error(
                "Synchronized schema changes require an operation transaction context".to_string(),
            ));
        }
        Ok(())
    }

    fn stage_index_drop(
        &self,
        space_id: u64,
        index_name: &str,
        schema_name: &str,
        index_type: &str,
        fields: &[String],
    ) -> Result<(), StorageError> {
        if !self.enabled {
            return Ok(());
        }
        let Some(sync_manager) = self.get_sync_manager() else {
            return Ok(());
        };
        let transaction_id = self.get_current_txn_id().ok_or_else(|| {
            StorageError::db_error(
                "Synchronized schema changes require an operation transaction context".to_string(),
            )
        })?;
        sync_manager
            .on_index_drop(
                transaction_id,
                space_id,
                index_name,
                schema_name,
                index_type,
                fields,
            )
            .map_err(|error| {
                StorageError::db_error(format!("Failed to stage index drop intent: {error}"))
            })
    }
}

pub mod admin;
pub mod operation;
pub mod persistence;
pub mod reader;
pub mod schema;
#[cfg(test)]
mod tests;
pub mod transaction;
pub mod undo;
mod write;
mod write_edge;
mod write_vertex;
