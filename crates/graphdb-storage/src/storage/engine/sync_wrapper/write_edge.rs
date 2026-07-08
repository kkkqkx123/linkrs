use super::SyncWrapper;
use crate::core::types::VertexId;
use crate::core::{Edge, StorageError, Value};
use crate::storage::{StorageClient, StorageTransactionContextOps};

impl<S: StorageClient + StorageTransactionContextOps + 'static> SyncWrapper<S> {
    pub(super) fn sync_insert_edge(
        &mut self,
        space: &str,
        edge: &Edge,
    ) -> Result<(), StorageError> {
        if !self.enabled {
            return Ok(());
        }

        let Some(sync_manager) = self.get_sync_manager() else {
            return Ok(());
        };

        let space_id = self.inner.get_space_id(space)?;
        let txn_id = self.get_current_txn_id();

        if let Some(txn_id) = txn_id {
            sync_manager
                .on_edge_insert(txn_id, space_id, edge)
                .map_err(|e| StorageError::db_error(format!("Failed to sync edge insert: {}", e)))
        } else {
            sync_manager
                .on_edge_insert_direct_sync(space_id, edge)
                .map_err(|e| StorageError::db_error(format!("Failed to sync edge insert: {}", e)))
        }
    }

    pub(super) fn sync_delete_edge(
        &mut self,
        space: &str,
        src: &VertexId,
        dst: &VertexId,
        edge_type: &str,
    ) -> Result<(), StorageError> {
        if !self.enabled {
            return Ok(());
        }

        let Some(sync_manager) = self.get_sync_manager() else {
            return Ok(());
        };

        let space_id = self.inner.get_space_id(space)?;
        let txn_id = self.get_current_txn_id();
        let src_value = Value::from(*src);
        let dst_value = Value::from(*dst);

        if let Some(txn_id) = txn_id {
            sync_manager
                .on_edge_delete(txn_id, space_id, &src_value, &dst_value, edge_type)
                .map_err(|e| StorageError::db_error(format!("Failed to sync edge delete: {}", e)))
        } else {
            sync_manager
                .on_edge_delete_direct_sync(space_id, &src_value, &dst_value, edge_type)
                .map_err(|e| StorageError::db_error(format!("Failed to sync edge delete: {}", e)))
        }
    }

    pub(super) fn sync_batch_insert_edges(
        &mut self,
        space: &str,
        edges: &[Edge],
    ) -> Result<(), StorageError> {
        if !self.enabled {
            return Ok(());
        }

        let Some(sync_manager) = self.get_sync_manager() else {
            return Ok(());
        };

        let space_id = self.inner.get_space_id(space)?;
        let txn_id = self.get_current_txn_id();

        for edge in edges {
            if let Some(txn_id) = txn_id {
                sync_manager
                    .on_edge_insert(txn_id, space_id, edge)
                    .map_err(|e| {
                        StorageError::db_error(format!("Failed to sync edge insert: {}", e))
                    })?;
            } else {
                sync_manager
                    .on_edge_insert_direct_sync(space_id, edge)
                    .map_err(|e| {
                        StorageError::db_error(format!("Failed to sync edge insert: {}", e))
                    })?;
            }
        }

        Ok(())
    }
}
