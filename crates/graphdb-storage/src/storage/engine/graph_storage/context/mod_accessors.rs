use crate::core::stats::StatsManager;
use crate::core::types::{LabelId, TableId, Timestamp};
use crate::storage::StorageOperationContext;
use std::sync::Arc;

use super::{GraphStorageContext, WriteTimestampLease};

impl GraphStorageContext {
    pub fn get_read_timestamp(&self) -> u32 {
        if let Some(operation) = &self.operation_context {
            operation.read_timestamp
        } else {
            self.persistent.version_manager.read_timestamp()
        }
    }

    pub fn get_write_timestamp(&self) -> u32 {
        if let Some(operation) = &self.operation_context {
            operation
                .write_timestamp
                .unwrap_or(operation.read_timestamp)
        } else {
            self.persistent.version_manager.next_write_timestamp()
        }
    }

    pub fn operation_context(&self) -> Option<Arc<StorageOperationContext>> {
        self.operation_context.clone()
    }

    pub fn with_operation_context(&self, context: StorageOperationContext) -> Self {
        let mut bound = self.clone();
        bound.operation_context = Some(Arc::new(context));
        bound.write_timestamp_lease = None;
        bound
    }

    pub fn with_auto_commit_context(&self) -> Self {
        let timestamp = self.persistent.version_manager.next_write_timestamp();
        let mut bound = self.clone();
        bound.operation_context = Some(Arc::new(StorageOperationContext {
            transaction_id: None,
            read_timestamp: timestamp,
            write_timestamp: Some(timestamp),
            read_only: false,
        }));
        bound.write_timestamp_lease = Some(Arc::new(WriteTimestampLease {
            version_manager: self.persistent.version_manager.clone(),
            timestamp,
        }));
        bound
    }

    pub(crate) fn release_write_timestamp(&self, timestamp: Timestamp) {
        if self.operation_context.is_none() {
            self.persistent
                .version_manager
                .release_insert_timestamp(timestamp);
        }
    }

    pub fn start_index_gc(&self) -> Option<std::thread::JoinHandle<()>> {
        self.runtime.start_index_gc()
    }

    pub fn stop_index_gc(&self) {
        self.runtime.stop_index_gc();
    }

    pub fn is_index_gc_running(&self) -> bool {
        self.runtime.is_index_gc_running()
    }

    pub fn mark_vertex_modified(&self, label: LabelId) {
        self.persistent
            .table_tracker
            .mark_modified(TableId::vertex(label));
    }

    pub fn mark_edge_modified(&self, label: LabelId) {
        self.persistent
            .table_tracker
            .mark_modified(TableId::edge(label));
    }

    pub(crate) fn storage_size(&self) -> usize {
        let mut total = 0usize;
        {
            let vertex_tables = self.persistent.data_store.read_vertex_tables();
            for table in vertex_tables.values() {
                total += table.memory_size();
            }
        }
        {
            let edge_tables = self.persistent.data_store.read_edge_tables();
            for table in edge_tables.values() {
                total += table.memory_size();
            }
        }
        total
    }

    pub(crate) fn used_storage_size(&self) -> usize {
        let mut total = 0usize;
        {
            let vertex_tables = self.persistent.data_store.read_vertex_tables();
            for table in vertex_tables.values() {
                total += table.used_memory_size();
            }
        }
        {
            let edge_tables = self.persistent.data_store.read_edge_tables();
            for table in edge_tables.values() {
                total += table.used_memory_size();
            }
        }
        total
    }

    pub(crate) fn is_open_flag(&self) -> &std::sync::atomic::AtomicBool {
        &self.persistent.is_open
    }

    pub(crate) fn index_data_manager(
        &self,
    ) -> &parking_lot::RwLock<crate::storage::index::IndexDataManagerImpl> {
        &self.persistent.index_data_manager
    }

    pub(crate) fn schema_manager(&self) -> &Arc<crate::core::metadata::SchemaManager> {
        &self.persistent.schema_manager
    }

    pub(crate) fn index_metadata_manager(&self) -> &Arc<crate::core::metadata::IndexManager> {
        &self.persistent.index_metadata_manager
    }

    pub(crate) fn version_manager(&self) -> &Arc<crate::transaction::VersionManager> {
        &self.persistent.version_manager
    }

    pub(crate) fn user_storage(&self) -> &Arc<crate::core::UserStorage> {
        &self.persistent.user_storage
    }

    pub(crate) fn persistence(
        &self,
    ) -> &Option<
        Arc<
            parking_lot::RwLock<
                crate::storage::engine::persistence_coordinator::PersistenceCoordinator,
            >,
        >,
    > {
        &self.persistent.persistence
    }

    pub(crate) fn stats_manager(&self) -> Option<&Arc<StatsManager>> {
        self.persistent.stats_manager.as_ref()
    }

    pub(crate) fn work_dir(&self) -> &Option<std::path::PathBuf> {
        self.persistent.layout.work_dir()
    }

    pub(crate) fn storage_paths(&self) -> Option<crate::storage::engine::paths::StoragePaths> {
        self.persistent.layout.storage_paths()
    }

    pub(crate) fn db_path(&self) -> &str {
        self.persistent.layout.db_path()
    }

    pub(crate) fn is_persistence_enabled(&self) -> bool {
        self.persistent.persistence.is_some()
    }

    pub(crate) fn data_store(&self) -> &Arc<crate::storage::engine::data_store::GraphDataStore> {
        &self.persistent.data_store
    }

    pub(crate) fn get_freeze_config_full(&self) -> crate::storage::engine::config::FreezeConfig {
        self.persistent.config.freeze.clone()
    }

    pub(crate) fn append_wal_redo<T: serde::Serialize>(
        &self,
        op_type: crate::core::wal::types::WalOpType,
        timestamp: Timestamp,
        redo: &T,
    ) -> crate::core::StorageResult<()> {
        if let Some(persistence) = self.persistent.persistence.as_ref() {
            let wal_manager = {
                let coordinator = persistence.read();
                coordinator.wal_manager()
            };
            if let Some(wal) = wal_manager {
                return wal.read().append_redo(op_type, timestamp, redo);
            }
        }

        Ok(())
    }

    pub(crate) fn defer_edge_insert(
        &self,
        edge: crate::core::wal::redo::InsertEdgeRedo,
        ts: Timestamp,
    ) {
        self.runtime.deferred_wal_ops.push_edge(edge, ts);
    }

    pub(crate) fn defer_edge_delete(
        &self,
        delete: crate::core::wal::redo::DeleteEdgeRedo,
        ts: Timestamp,
    ) {
        self.runtime.deferred_wal_ops.push_delete(delete, ts);
    }

    pub(crate) fn take_deferred_edge_inserts(
        &self,
    ) -> Vec<(crate::core::wal::redo::InsertEdgeRedo, Timestamp)> {
        self.runtime.deferred_wal_ops.drain_edges()
    }

    pub(crate) fn take_deferred_edge_deletes(
        &self,
    ) -> Vec<(crate::core::wal::redo::DeleteEdgeRedo, Timestamp)> {
        self.runtime.deferred_wal_ops.drain_deletes()
    }
}
