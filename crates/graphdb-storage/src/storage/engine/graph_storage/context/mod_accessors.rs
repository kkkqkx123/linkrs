use crate::core::stats::StatsManager;
use crate::core::types::{LabelId, TableId, Timestamp};
use crate::storage::engine::resource_budget::{MemoryCategory, ResourceSnapshot};
use crate::storage::index::IndexGcOps;
use crate::storage::StorageOperationContext;
use std::sync::atomic::Ordering;
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
        let transaction_id = crate::core::types::TransactionId::new(
            self.persistent
                .next_auto_transaction_id
                .fetch_add(1, Ordering::SeqCst),
        );
        let mut bound = self.clone();
        bound.operation_context = Some(Arc::new(StorageOperationContext {
            transaction_id: Some(transaction_id),
            read_timestamp: timestamp,
            write_timestamp: Some(timestamp),
            read_only: false,
            auto_commit: true,
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
                .release_write_timestamp(timestamp);
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
        let vertices = self.persistent.data_store.with_vertex_tables(|tables| {
            tables
                .values()
                .map(|table| table.memory_size())
                .sum::<usize>()
        });
        let edges = self.persistent.data_store.with_edge_tables(|tables| {
            tables
                .values()
                .map(|table| table.memory_size())
                .sum::<usize>()
        });
        vertices + edges
    }

    pub(crate) fn used_storage_size(&self) -> usize {
        let vertices = self.persistent.data_store.with_vertex_tables(|tables| {
            tables
                .values()
                .map(|table| table.used_memory_size())
                .sum::<usize>()
        });
        let edges = self.persistent.data_store.with_edge_tables(|tables| {
            tables
                .values()
                .map(|table| table.used_memory_size())
                .sum::<usize>()
        });
        vertices + edges
    }

    pub fn resource_snapshot(&self) -> ResourceSnapshot {
        self.persistent
            .resource_accounting
            .report_usage(MemoryCategory::Data, self.used_storage_size() as u64);
        let index_bytes = self
            .persistent
            .index_data_manager
            .read()
            .memory_usage_bytes();
        self.persistent
            .resource_accounting
            .report_usage(MemoryCategory::Index, index_bytes);
        let tombstone_count = self.persistent.index_data_manager.read().tombstone_count();
        let tombstone_memory_bytes = (tombstone_count as u64).saturating_mul(64);
        self.persistent
            .resource_accounting
            .report_usage(MemoryCategory::Mvcc, tombstone_memory_bytes);
        let _ = self.persistent.cache_manager.refresh_memory_usage();
        let mut snapshot = self.persistent.resource_accounting.snapshot();
        snapshot.active_snapshots = self
            .persistent
            .version_manager
            .snapshot_tracker()
            .active_count();
        snapshot.oldest_snapshot_ts = self
            .persistent
            .version_manager
            .snapshot_tracker()
            .cleanup_threshold();
        snapshot.tombstone_count = tombstone_count;
        snapshot.tombstone_memory_bytes = tombstone_memory_bytes;
        snapshot
    }

    pub fn check_write_admission(&self) -> crate::core::StorageResult<()> {
        let snapshot = self.resource_snapshot();
        if snapshot.hard_limit_exceeded() {
            return Err(crate::core::StorageError::capacity_exceeded());
        }
        let resources = &self.persistent.config.resources;
        if snapshot.tombstone_count >= resources.max_tombstones
            || snapshot.tombstone_memory_bytes >= resources.max_tombstone_bytes
        {
            return Err(crate::core::StorageError::capacity_exceeded());
        }
        if snapshot.soft_limit_exceeded() {
            log::debug!(
                "Storage memory is above the soft limit: {} / {} bytes",
                snapshot.total_current_bytes,
                snapshot.budget.max_memory_bytes
            );
        }
        Ok(())
    }

    pub fn check_snapshot_admission(&self) -> crate::core::StorageResult<()> {
        let active = self
            .persistent
            .version_manager
            .snapshot_tracker()
            .active_count();
        if active >= self.persistent.config.resources.max_active_snapshots {
            return Err(crate::core::StorageError::capacity_exceeded());
        }
        Ok(())
    }

    pub fn wal_metrics(&self) -> Option<crate::storage::WalMetrics> {
        let persistence = self.persistent.persistence.as_ref()?;
        let wal = persistence.read().wal_manager()?;
        let metrics = wal.read().metrics();
        Some(metrics)
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
        if let Some(transaction_id) = self
            .operation_context
            .as_ref()
            .and_then(|operation| operation.transaction_id)
        {
            let payload = postcard::to_allocvec(redo).map_err(|error| {
                crate::core::StorageError::serialize_error(format!(
                    "Failed to serialize staged WAL redo: {}",
                    error
                ))
            })?;
            self.persistent
                .staged_wal
                .entry(transaction_id)
                .or_default()
                .push(crate::transaction::wal::TransactionWalEntry {
                    op_type,
                    timestamp,
                    payload,
                });
            return Ok(());
        }
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

    pub(crate) fn commit_staged_writes(
        &self,
        transaction_id: crate::core::types::TransactionId,
        intents: &[crate::core::wal::OutboxIntent],
    ) -> crate::core::StorageResult<crate::core::types::CommitLsn> {
        let entries = self
            .persistent
            .staged_wal
            .get(&transaction_id)
            .map(|entries| entries.clone())
            .unwrap_or_default();
        let commit_lsn = if let Some(persistence) = self.persistent.persistence.as_ref() {
            let wal_manager = persistence.read().wal_manager().ok_or_else(|| {
                crate::core::StorageError::wal_error("WAL manager is not initialized".to_string())
            })?;
            let result = wal_manager
                .read()
                .append_transaction(transaction_id, entries, intents)?;
            result
        } else {
            crate::core::types::CommitLsn::ZERO
        };
        self.persistent.staged_wal.remove(&transaction_id);
        Ok(commit_lsn)
    }

    pub(crate) fn abort_staged_writes(&self, transaction_id: crate::core::types::TransactionId) {
        self.persistent.staged_wal.remove(&transaction_id);
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
