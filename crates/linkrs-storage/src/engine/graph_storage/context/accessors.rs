use std::sync::atomic::Ordering;
use std::sync::Arc;

use linkrs_core::types::{LabelId, TableId, Timestamp};
use linkrs_core::{StorageError, StorageResult};
use linkrs_metrics::StatsManager;

use crate::StorageOperationContext;

use super::{GraphStorageContext, WriteTimestampLease};

impl GraphStorageContext {
    pub fn get_read_timestamp(&self) -> Timestamp {
        if let Some(operation) = &self.operation_context {
            operation.read_timestamp
        } else {
            self.persistent.version_manager.read_timestamp()
        }
    }

    pub fn get_write_timestamp(&self) -> StorageResult<Timestamp> {
        if let Some(operation) = &self.operation_context {
            if operation.read_only {
                return Err(StorageError::invalid_operation(
                    "Read-only transaction cannot perform writes",
                ));
            }
            operation.write_timestamp.ok_or_else(|| {
                StorageError::db_error("No write timestamp is available for this operation")
            })
        } else {
            self.persistent
                .version_manager
                .try_next_write_timestamp()
                .map_err(|error| StorageError::db_error(error.to_string()))
        }
    }

    pub fn operation_context(&self) -> Option<Arc<StorageOperationContext>> {
        self.operation_context.clone()
    }

    pub fn mutation_recorder(
        &self,
    ) -> Option<Arc<dyn linkrs_transaction::TransactionMutationRecorder>> {
        self.operation_context
            .as_ref()
            .and_then(|context| context.mutation_recorder.clone())
    }

    pub fn with_operation_context(&self, context: StorageOperationContext) -> Self {
        let mut bound = self.clone();
        bound.operation_context = Some(Arc::new(context));
        bound.write_timestamp_lease = None;
        bound.write_gate_lease = None;
        bound.auto_commit_window = None;
        bound.auto_commit_write_set = None;
        bound
    }

    pub fn with_read_operation_context(&self) -> StorageResult<Self> {
        let timestamp = self.persistent.version_manager.read_timestamp();
        let mut bound = self.clone();
        bound.operation_context = Some(Arc::new(StorageOperationContext {
            transaction_id: None,
            read_timestamp: timestamp,
            write_timestamp: None,
            read_only: true,
            auto_commit: true,
            mutation_recorder: None,
            auto_commit_group_start: None,
            auto_commit_staging_start: None,
            auto_commit_wal_start: 0,
        }));
        Ok(bound)
    }

    pub fn with_auto_commit_context(&self) -> StorageResult<Self> {
        let write_gate_lease = self.persistent.auto_commit_write_gate.acquire();
        let timestamp = self
            .persistent
            .version_manager
            .try_next_write_timestamp()
            .map_err(|error| StorageError::db_error(error.to_string()))?;
        let transaction_id = linkrs_core::types::TransactionId::new(
            self.persistent
                .next_auto_transaction_id
                .fetch_add(1, Ordering::SeqCst),
        );

        let mut bound = self.clone();
        let undo_log = Arc::new(parking_lot::Mutex::new(
            linkrs_transaction::UndoLogManager::new(),
        ));
        let write_set = Arc::new(parking_lot::Mutex::new(
            linkrs_transaction::types::WriteSet::new(),
        ));
        let context = StorageOperationContext {
            transaction_id: Some(transaction_id),
            read_timestamp: timestamp,
            write_timestamp: Some(timestamp),
            read_only: false,
            auto_commit: true,
            mutation_recorder: Some(Arc::new(super::AutoCommitMutationRecorder {
                undo: undo_log.clone(),
                write_set: write_set.clone(),
            })),
            auto_commit_group_start: None,
            auto_commit_staging_start: None,
            auto_commit_wal_start: 0,
        };

        bound.operation_context = Some(Arc::new(context));
        bound.write_timestamp_lease = Some(Arc::new(WriteTimestampLease {
            version_manager: self.persistent.version_manager.clone(),
            timestamp,
            finalized: std::sync::atomic::AtomicBool::new(false),
        }));
        bound.write_gate_lease = Some(write_gate_lease);
        bound.auto_commit_undo = Some(undo_log);
        bound.auto_commit_write_set = Some(write_set);
        Ok(bound)
    }

    pub fn start_index_gc(&self) -> Option<crate::thread_pool::BackgroundTaskHandle> {
        self.runtime.start_index_gc()
    }

    pub(crate) fn maybe_run_index_gc(&self) {
        self.runtime.maybe_run_index_gc();
    }

    pub fn stop_index_gc(&self) {
        self.runtime.stop_index_gc();
    }

    pub fn is_index_gc_running(&self) -> bool {
        self.runtime.is_index_gc_running()
    }

    pub fn start_vertex_gc(&self) -> Option<crate::thread_pool::BackgroundTaskHandle> {
        self.runtime.start_vertex_gc()
    }

    pub fn stop_vertex_gc(&self) {
        self.runtime.stop_vertex_gc();
    }

    pub fn is_vertex_gc_running(&self) -> bool {
        self.runtime.is_vertex_gc_running()
    }

    pub fn mark_vertex_modified(&self, label: LabelId) {
        self.persistent
            .table_tracker
            .mark_modified(TableId::vertex(label));
    }

    /// Record-cache bookkeeping for a freshly inserted vertex ID. Writer
    /// paths outside the context module use this instead of reaching into
    /// the persistent state directly.
    pub(crate) fn cache_inserted_vertex_id(
        &self,
        label: LabelId,
        external_id: &str,
        internal_id: u32,
        ts: Timestamp,
    ) {
        self.persistent
            .cache_manager
            .cache_vertex_id(label, external_id, internal_id, ts);
    }

    pub fn mark_edge_modified(&self, label: LabelId) {
        self.persistent
            .table_tracker
            .mark_modified(TableId::edge(label));
    }

    pub fn wal_metrics(&self) -> Option<crate::WalMetrics> {
        let persistence = self.persistent.persistence.as_ref()?;
        let wal = persistence.read().wal_manager()?;
        let metrics = wal.read().metrics();
        Some(metrics)
    }

    /// Force the WAL to a durable state. Write-ahead semantics require a redo
    /// record to be durable before the operation that produced it is
    /// acknowledged; the async flush buffer alone cannot guarantee that.
    /// No-op when persistence is disabled.
    pub(crate) fn sync_wal(&self) -> StorageResult<()> {
        if let Some(persistence) = self.persistent.persistence.as_ref() {
            if let Some(wal) = persistence.read().wal_manager() {
                wal.read().sync()?;
            }
        }
        Ok(())
    }

    pub(crate) fn is_open_flag(&self) -> &std::sync::atomic::AtomicBool {
        &self.persistent.is_open
    }

    pub(crate) fn index_data_manager(
        &self,
    ) -> &parking_lot::RwLock<crate::index::IndexDataManagerImpl> {
        &self.persistent.index_data_manager
    }

    pub(crate) fn schema_manager(&self) -> &Arc<linkrs_core::metadata::SchemaManager> {
        &self.persistent.schema_manager
    }

    pub(crate) fn serial_allocator(
        &self,
    ) -> &crate::engine::graph_storage::serial::SerialAllocator {
        &self.persistent.serial_allocator
    }

    pub(crate) fn index_metadata_manager(&self) -> &Arc<linkrs_core::metadata::IndexManager> {
        &self.persistent.index_metadata_manager
    }

    pub(crate) fn version_manager(&self) -> &Arc<linkrs_transaction::VersionManager> {
        &self.persistent.version_manager
    }

    pub(crate) fn user_storage(&self) -> &Arc<linkrs_core::UserStorage> {
        &self.persistent.user_storage
    }

    /// Hash partitions per vertex label table (auto-compaction / ID space
    /// related config).
    pub(crate) fn vertex_table_shards(&self) -> usize {
        self.persistent.config.vertex_table_shards
    }

    pub(crate) fn persistence(
        &self,
    ) -> &Option<
        Arc<parking_lot::RwLock<crate::engine::persistence_coordinator::PersistenceCoordinator>>,
    > {
        &self.persistent.persistence
    }

    pub(crate) fn stats_manager(&self) -> Option<&Arc<StatsManager>> {
        self.persistent.stats_manager.as_ref()
    }

    pub(crate) fn work_dir(&self) -> &Option<std::path::PathBuf> {
        self.persistent.layout.work_dir()
    }

    /// Compression used for baseline flushes, so offline tools write probe
    /// and checkpoint content identically to the regular flush path.
    pub(crate) fn flush_compression(&self) -> crate::compression::CompressionType {
        self.persistent.config.flush_config.compression
    }

    pub(crate) fn storage_paths(&self) -> Option<crate::engine::paths::StoragePaths> {
        self.persistent.layout.storage_paths()
    }

    pub(crate) fn db_path(&self) -> &str {
        self.persistent.layout.db_path()
    }

    pub(crate) fn is_persistence_enabled(&self) -> bool {
        self.persistent.persistence.is_some()
    }

    pub(crate) fn data_store(&self) -> &Arc<crate::engine::data_store::GraphDataStore> {
        &self.persistent.data_store
    }

    pub(crate) fn spiller(&self) -> &Arc<crate::engine::spiller::Spiller> {
        &self.persistent.spiller
    }

    pub fn try_reserve_with_spill(
        &self,
        category: crate::engine::resource_budget::MemoryCategory,
        bytes: u64,
    ) -> linkrs_core::StorageResult<crate::engine::resource_budget::MemoryReservation> {
        self.persistent
            .spiller
            .try_reserve_with_spill(category, bytes)
    }

    pub(crate) fn get_freeze_config_full(&self) -> crate::engine::config::FreezeConfig {
        self.persistent.config.freeze.clone()
    }
}
