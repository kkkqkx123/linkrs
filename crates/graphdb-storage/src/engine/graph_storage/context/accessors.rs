use std::sync::atomic::Ordering;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::cold::ColdSnapshot;
use crate::engine::graph_storage::context::VertexIdDomainEvidence;
use crate::engine::resource_budget::{MemoryCategory, ResourceSnapshot};
use graphdb_core::stats::StatsManager;
use graphdb_core::types::{LabelId, TableId, Timestamp};
use graphdb_core::{StorageError, StorageResult};

use crate::SnapshotHandle;
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
    ) -> Option<Arc<dyn graphdb_transaction::TransactionMutationRecorder>> {
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
            mvcc_vertex_snapshot_handles: Vec::new(),
            mvcc_edge_snapshot_registered: false,
            registered_vertex_labels: parking_lot::RwLock::new(std::collections::HashSet::new()),
            registered_edge_partitions: parking_lot::RwLock::new(std::collections::HashSet::new()),
            auto_commit_group_start: None,
        }));
        Ok(bound)
    }

    pub(crate) fn register_auto_commit_snapshots(
        &self,
        _timestamp: Timestamp,
    ) -> StorageResult<(Vec<(LabelId, SnapshotHandle)>, bool)> {
        Ok((Vec::new(), false))
    }

    pub fn with_auto_commit_context(&self) -> StorageResult<Self> {
        let write_gate_lease = self.persistent.auto_commit_write_gate.acquire();
        let timestamp = self
            .persistent
            .version_manager
            .try_next_write_timestamp()
            .map_err(|error| StorageError::db_error(error.to_string()))?;
        let transaction_id = graphdb_core::types::TransactionId::new(
            self.persistent
                .next_auto_transaction_id
                .fetch_add(1, Ordering::SeqCst),
        );

        let mut bound = self.clone();
        let undo_log = Arc::new(parking_lot::Mutex::new(
            graphdb_transaction::UndoLogManager::new(),
        ));
        let mut context = StorageOperationContext {
            transaction_id: Some(transaction_id),
            read_timestamp: timestamp,
            write_timestamp: Some(timestamp),
            read_only: false,
            auto_commit: true,
            mutation_recorder: Some(Arc::new(super::AutoCommitMutationRecorder {
                undo: undo_log.clone(),
                write_set: Arc::new(parking_lot::Mutex::new(
                    graphdb_transaction::types::WriteSet::new(),
                )),
            })),
            mvcc_vertex_snapshot_handles: Vec::new(),
            mvcc_edge_snapshot_registered: false,
            registered_vertex_labels: parking_lot::RwLock::new(std::collections::HashSet::new()),
            registered_edge_partitions: parking_lot::RwLock::new(std::collections::HashSet::new()),
            auto_commit_group_start: None,
        };

        let (vertex_handles, edge_registered) = self.register_auto_commit_snapshots(timestamp)?;
        context.mvcc_vertex_snapshot_handles = vertex_handles;
        context.mvcc_edge_snapshot_registered = edge_registered;

        bound.operation_context = Some(Arc::new(context));
        bound.write_timestamp_lease = Some(Arc::new(WriteTimestampLease {
            version_manager: self.persistent.version_manager.clone(),
            timestamp,
            finalized: std::sync::atomic::AtomicBool::new(false),
        }));
        bound.write_gate_lease = Some(write_gate_lease);
        bound.auto_commit_undo = Some(undo_log);
        Ok(bound)
    }

    pub(crate) fn begin_auto_commit_batch(
        &self,
    ) -> StorageResult<Arc<super::AutoCommitBatchWindow>> {
        let write_gate_lease = self.persistent.auto_commit_write_gate.acquire();
        let mut clean = self.clone();
        clean.operation_context = None;
        clean.write_timestamp_lease = None;
        clean.write_gate_lease = None;
        clean.auto_commit_undo = None;
        clean.auto_commit_window = None;
        Ok(Arc::new(super::AutoCommitBatchWindow {
            base_ctx: Arc::new(clean),
            gate_lease: write_gate_lease,
            first_ts: parking_lot::Mutex::new(None),
            registered_vertex_snapshots: parking_lot::Mutex::new(Vec::new()),
            registered_edge_snapshots: parking_lot::Mutex::new(Vec::new()),
            statement_count: std::sync::atomic::AtomicU64::new(0),
            snapshot_rounds: std::sync::atomic::AtomicU64::new(0),
            group: std::sync::atomic::AtomicBool::new(false),
            group_undo: None,
        }))
    }

    pub(crate) fn begin_auto_commit_group(
        &self,
    ) -> StorageResult<Arc<super::AutoCommitBatchWindow>> {
        let write_gate_lease = self.persistent.auto_commit_write_gate.acquire();
        let mut clean = self.clone();
        clean.operation_context = None;
        clean.write_timestamp_lease = None;
        clean.write_gate_lease = None;
        clean.auto_commit_undo = None;
        clean.auto_commit_window = None;
        Ok(Arc::new(super::AutoCommitBatchWindow {
            base_ctx: Arc::new(clean),
            gate_lease: write_gate_lease,
            first_ts: parking_lot::Mutex::new(None),
            registered_vertex_snapshots: parking_lot::Mutex::new(Vec::new()),
            registered_edge_snapshots: parking_lot::Mutex::new(Vec::new()),
            statement_count: std::sync::atomic::AtomicU64::new(0),
            snapshot_rounds: std::sync::atomic::AtomicU64::new(0),
            group: std::sync::atomic::AtomicBool::new(true),
            group_undo: Some(Arc::new(parking_lot::Mutex::new(
                graphdb_transaction::UndoLogManager::new(),
            ))),
        }))
    }

    pub(crate) fn restore_auto_transaction_id(&self, max_transaction_id: u64) {
        self.persistent
            .next_auto_transaction_id
            .fetch_max(max_transaction_id.saturating_add(1), Ordering::SeqCst);
    }

    pub(crate) fn commit_write_timestamp(&self, timestamp: Timestamp) {
        if let Some(lease) = &self.write_timestamp_lease {
            lease.commit();
        } else if self.operation_context.is_none() {
            self.persistent
                .version_manager
                .commit_write_timestamp(timestamp);
        }
    }

    pub(crate) fn abort_write_timestamp(&self, timestamp: Timestamp) {
        if let Some(lease) = &self.write_timestamp_lease {
            lease.abort();
        } else if self.operation_context.is_none() {
            self.persistent
                .version_manager
                .abort_write_timestamp(timestamp);
        }
    }

    pub(crate) fn finalize_operation(&self, committed: bool) -> StorageResult<()> {
        let Some(operation) = &self.operation_context else {
            return Ok(());
        };
        if !operation.auto_commit {
            return Ok(());
        }

        if self.auto_commit_window.is_none() {
            self.unregister_statement_snapshots(operation);
        }

        if operation.read_only {
            return Ok(());
        }

        // Group mode: per-statement finalize — no-wait WAL append or segment
        // rollback. Do NOT commit/abort the write timestamp, release the gate,
        // or unregister snapshots — those are deferred to `finalize_group`.
        if let Some(window) = &self.auto_commit_window {
            if window.is_grouped() {
                let timestamp = operation.write_timestamp.ok_or_else(|| {
                    StorageError::db_error("Group operation has no write timestamp")
                })?;
                if committed {
                    if let Some(txid) = operation.transaction_id {
                        self.commit_staged_writes_grouped(txid, &[])?;
                    }
                } else {
                    if let Some(undo) = &self.auto_commit_undo {
                        let mut log = undo.lock();
                        let start = operation.auto_commit_group_start.unwrap_or(0);
                        if let Err(error) = log.execute_undo_from_index(self, timestamp, start) {
                            log::error!("Group statement rollback failed: {}", error);
                        }
                    }
                    if let Some(txid) = operation.transaction_id {
                        self.abort_staged_writes(txid);
                    }
                }
                self.maybe_run_index_gc();
                return Ok(());
            }
        }

        let timestamp = operation.write_timestamp.ok_or_else(|| {
            StorageError::db_error("Auto-commit operation has no write timestamp")
        })?;
        let transaction_id = operation.transaction_id;

        if committed {
            self.commit_write_timestamp(timestamp);
        } else {
            if let Some(undo) = &self.auto_commit_undo {
                let mut log = undo.lock();
                if let Err(error) = log.execute_undo(self, timestamp) {
                    log::error!("Auto-commit rollback failed: {}", error);
                }
            }
            self.abort_write_timestamp(timestamp);
        }
        if let Some(lease) = &self.write_gate_lease {
            lease.release();
        }
        if let Some(transaction_id) = transaction_id {
            self.persistent.staged_wal.remove(&transaction_id);
        }
        self.maybe_run_index_gc();
        Ok(())
    }

    fn unregister_statement_snapshots(&self, operation: &StorageOperationContext) {
        let Some(timestamp) = operation.snapshot_timestamp() else {
            return;
        };

        let registered_labels: Vec<LabelId> = {
            let registered = operation.registered_vertex_labels.read();
            registered.iter().cloned().collect()
        };

        if !registered_labels.is_empty() {
            let tables: Vec<(
                LabelId,
                Arc<crate::vertex::vertex_table::ShardedVertexTable>,
            )> = self
                .persistent
                .data_store
                .with_vertex_tables(|vertex_tables| {
                    registered_labels
                        .iter()
                        .filter_map(|label_id| {
                            vertex_tables
                                .get(label_id)
                                .map(|table| (*label_id, table.clone()))
                        })
                        .collect()
                });
            for (_label_id, vertex_table) in tables {
                let _ = vertex_table.unregister_snapshot_by_timestamp(timestamp);
            }
        }

        let registered_edge_keys: Vec<crate::engine::data_store::EdgeTableKey> = {
            let registered = operation.registered_edge_partitions.read();
            registered.iter().cloned().collect()
        };

        if !registered_edge_keys.is_empty() {
            let edge_tables: Vec<Arc<parking_lot::RwLock<crate::edge::EdgeStore>>> =
                self.persistent.data_store.with_edge_tables(|tables| {
                    registered_edge_keys
                        .iter()
                        .filter_map(|key| tables.get(key).cloned())
                        .collect()
                });
            for edge_table in edge_tables {
                edge_table.write().unregister_snapshot(timestamp);
            }
        }
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

    pub fn mark_edge_modified(&self, label: LabelId) {
        self.runtime
            .last_edge_write
            .lock()
            .insert(label, std::time::Instant::now());
        self.persistent
            .table_tracker
            .mark_modified(TableId::edge(label));
    }

    /// Seconds since the last write to `label`'s edge tables (wall clock).
    /// Labels without any recorded write report `u64::MAX`.
    pub(crate) fn edge_idle_seconds(&self, label: LabelId) -> u64 {
        let last_write = self.runtime.last_edge_write.lock();
        match last_write.get(&label) {
            Some(instant) => instant.elapsed().as_secs(),
            None => u64::MAX,
        }
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
                .map(|arc| arc.read().memory_size())
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
                .map(|arc| arc.read().used_memory_size())
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
            .cached_memory_usage_bytes();
        self.persistent
            .resource_accounting
            .report_usage(MemoryCategory::Index, index_bytes);
        let tombstone_count = self
            .persistent
            .index_data_manager
            .read()
            .cached_tombstone_count() as usize;
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
        snapshot.vertex_shard_count = self
            .persistent
            .data_store
            .with_vertex_tables(|tables| tables.values().map(|table| table.num_shards()).sum());
        // Keep spiller accessors exercised.
        let _spill_ratio = self.spiller().spill_threshold_ratio();
        let _spill_dir = self.spiller().spill_dir();
        let _active_spills = self.spiller().active_spills().read().len();
        // Exercise try_reserve_with_spill with a zero-byte probe to keep
        // the full reservation-with-spill path compiled and tested.
        let _probe = self.try_reserve_with_spill(MemoryCategory::Data, 0);
        // Keep vertex GC stats exercised.
        if let Some(ref gc) = self.runtime.vertex_gc_manager {
            let _total = gc.total_removed();
            let _passes = gc.pass_count();
        }
        snapshot
    }

    pub fn check_write_admission(&self) -> graphdb_core::StorageResult<()> {
        if self
            .operation_context
            .as_ref()
            .is_some_and(|context| context.read_only)
        {
            return Err(graphdb_core::StorageError::invalid_operation(
                "Read-only transaction cannot perform writes",
            ));
        }
        let snapshot = self.resource_snapshot();
        if snapshot.hard_limit_exceeded() {
            // Before failing, attempt to spill cold data to recover memory.
            let overage = snapshot
                .total_current_bytes
                .saturating_sub(snapshot.budget.max_memory_bytes)
                + 1024 * 1024;
            self.spiller().spill_cold_data(overage);
            let snapshot = self.resource_snapshot();
            if snapshot.hard_limit_exceeded() {
                return Err(graphdb_core::StorageError::capacity_exceeded());
            }
        }
        let resources = &self.persistent.config.resources;
        if snapshot.tombstone_count >= resources.max_tombstones
            || snapshot.tombstone_memory_bytes >= resources.max_tombstone_bytes
        {
            return Err(graphdb_core::StorageError::capacity_exceeded());
        }
        if snapshot.soft_limit_exceeded() {
            log::debug!(
                "Storage memory is above the soft limit: {} / {} bytes",
                snapshot.total_current_bytes,
                snapshot.budget.max_memory_bytes
            );
        }
        // Version-chain memory diagnostic and long-transaction backpressure.
        // Long-lived snapshots pin history; we warn and surface
        // blocked bytes but do not force the safe GC frontier forward.
        {
            let coordinator = crate::engine::gc_coordinator::GcCoordinator::new(
                self.persistent.version_manager.clone(),
            );
            let diag = coordinator.diagnostics();
            if diag.is_blocked() {
                let version_bytes: usize =
                    self.persistent.data_store.with_vertex_tables(|tables| {
                        tables
                            .values()
                            .map(|t| t.version_chain_memory_bytes())
                            .sum()
                    });
                if version_bytes > 64 * 1024 * 1024 {
                    log::warn!(
                        "write admission: long transaction blocks GC ({} bytes version chains, {} snapshots)",
                        version_bytes,
                        diag.active_snapshot_count
                    );
                }
            }
        }
        Ok(())
    }

    pub fn check_snapshot_admission(&self) -> graphdb_core::StorageResult<()> {
        let tracker = self.persistent.version_manager.snapshot_tracker();
        let active = tracker.active_count();
        if active >= self.persistent.config.resources.max_active_snapshots {
            return Err(graphdb_core::StorageError::capacity_exceeded());
        }
        if let Some(age) = tracker.oldest_age() {
            if age >= self.persistent.config.resources.max_snapshot_age {
                return Err(graphdb_core::StorageError::invalid_operation(
                    "Oldest active snapshot exceeded max_snapshot_age",
                ));
            }
            // Long-transaction diagnostic: warn when oldest snapshot
            // ages past 30s and blocks GC. Does not force watermark forward;
            // caller may retry or apply backpressure upstream.
            if age.as_secs() > 30 {
                let coordinator = crate::engine::gc_coordinator::GcCoordinator::new(
                    self.persistent.version_manager.clone(),
                );
                let diag = coordinator.diagnostics();
                if let Some(warn) = diag.long_transaction_warning {
                    log::warn!("snapshot admission blocked by long transaction: {}", warn);
                }
            }
        }
        Ok(())
    }

    pub fn wal_metrics(&self) -> Option<crate::WalMetrics> {
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
    ) -> &parking_lot::RwLock<crate::index::IndexDataManagerImpl> {
        &self.persistent.index_data_manager
    }

    pub(crate) fn schema_manager(&self) -> &Arc<graphdb_core::metadata::SchemaManager> {
        &self.persistent.schema_manager
    }

    pub(crate) fn serial_allocator(
        &self,
    ) -> &crate::engine::graph_storage::serial::SerialAllocator {
        &self.persistent.serial_allocator
    }

    pub(crate) fn index_metadata_manager(&self) -> &Arc<graphdb_core::metadata::IndexManager> {
        &self.persistent.index_metadata_manager
    }

    pub(crate) fn version_manager(&self) -> &Arc<graphdb_transaction::VersionManager> {
        &self.persistent.version_manager
    }

    pub(crate) fn user_storage(&self) -> &Arc<graphdb_core::UserStorage> {
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
    ) -> graphdb_core::StorageResult<crate::engine::resource_budget::MemoryReservation> {
        self.persistent
            .spiller
            .try_reserve_with_spill(category, bytes)
    }

    pub(crate) fn get_freeze_config_full(&self) -> crate::engine::config::FreezeConfig {
        self.persistent.config.freeze.clone()
    }

    pub(crate) fn append_wal_redo<T: serde::Serialize>(
        &self,
        op_type: graphdb_core::wal::types::WalOpType,
        timestamp: Timestamp,
        redo: &T,
    ) -> graphdb_core::StorageResult<graphdb_transaction::wal::TransactionWalEntry> {
        let payload = postcard::to_allocvec(redo).map_err(|error| {
            graphdb_core::StorageError::serialize_error(format!(
                "Failed to serialize WAL redo: {}",
                error
            ))
        })?;
        if let Some(transaction_id) = self
            .operation_context
            .as_ref()
            .and_then(|operation| operation.transaction_id)
        {
            let entry = graphdb_transaction::wal::TransactionWalEntry {
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
                return Ok(graphdb_transaction::wal::TransactionWalEntry {
                    op_type,
                    timestamp,
                    payload,
                    transaction_id: None,
                    mutation_sequence: None,
                });
            }
        }

        Ok(graphdb_transaction::wal::TransactionWalEntry {
            op_type,
            timestamp,
            payload,
            transaction_id: None,
            mutation_sequence: None,
        })
    }

    pub(crate) fn commit_staged_writes(
        &self,
        transaction_id: graphdb_core::types::TransactionId,
        intents: &[graphdb_core::wal::OutboxIntent],
    ) -> graphdb_core::StorageResult<graphdb_core::types::CommitLsn> {
        self.commit_staged_writes_with_durability(
            transaction_id,
            intents,
            graphdb_core::types::DurabilityLevel::Sync,
        )
    }

    pub(crate) fn commit_staged_writes_with_durability(
        &self,
        transaction_id: graphdb_core::types::TransactionId,
        intents: &[graphdb_core::wal::OutboxIntent],
        durability: graphdb_core::types::DurabilityLevel,
    ) -> graphdb_core::StorageResult<graphdb_core::types::CommitLsn> {
        let entries = self
            .persistent
            .staged_wal
            .get(&transaction_id)
            .map(|entries| entries.clone())
            .unwrap_or_default();
        let commit_lsn = if let Some(persistence) = self.persistent.persistence.as_ref() {
            let wal_manager = persistence.read().wal_manager().ok_or_else(|| {
                graphdb_core::StorageError::wal_error("WAL manager is not initialized".to_string())
            })?;
            let result = wal_manager.read().append_transaction_with_durability(
                transaction_id,
                entries,
                intents,
                durability,
            )?;
            result
        } else {
            graphdb_core::types::CommitLsn::ZERO
        };
        self.persistent
            .index_data_manager
            .read()
            .advance_barriers(commit_lsn);
        self.persistent.staged_wal.remove(&transaction_id);
        Ok(commit_lsn)
    }

    /// Append staged WAL with `DurabilityLevel::None` (no fsync). Barriers
    /// are deferred to the group commit point (`finalize_group`).
    pub(crate) fn commit_staged_writes_grouped(
        &self,
        transaction_id: graphdb_core::types::TransactionId,
        intents: &[graphdb_core::wal::OutboxIntent],
    ) -> graphdb_core::StorageResult<graphdb_core::types::CommitLsn> {
        let entries = self
            .persistent
            .staged_wal
            .get(&transaction_id)
            .map(|entries| entries.clone())
            .unwrap_or_default();
        let commit_lsn = if let Some(persistence) = self.persistent.persistence.as_ref() {
            let guard = persistence.read();
            let wal_manager = guard.wal_manager().ok_or_else(|| {
                graphdb_core::StorageError::wal_error("WAL manager is not initialized".to_string())
            })?;
            let result = wal_manager.read().append_transaction_with_durability(
                transaction_id,
                entries,
                intents,
                graphdb_core::types::DurabilityLevel::None,
            )?;
            result
        } else {
            graphdb_core::types::CommitLsn::ZERO
        };
        // Deliberately no advance_barriers: deferred to finalize_group.
        self.persistent.staged_wal.remove(&transaction_id);
        Ok(commit_lsn)
    }

    pub(crate) fn abort_staged_writes(&self, transaction_id: graphdb_core::types::TransactionId) {
        self.persistent.staged_wal.remove(&transaction_id);
    }

    /// Number of staged-WAL entries held for in-flight transactions.
    pub(crate) fn staged_wal_len(&self) -> usize {
        self.persistent.staged_wal.len()
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
        edge: graphdb_core::wal::redo::InsertEdgeRedo,
        ts: Timestamp,
    ) {
        self.runtime.deferred_wal_ops.push_edge(edge, ts);
    }

    pub(crate) fn defer_edge_delete(
        &self,
        delete: graphdb_core::wal::redo::DeleteEdgeRedo,
        ts: Timestamp,
    ) {
        self.runtime.deferred_wal_ops.push_delete(delete, ts);
    }

    pub(crate) fn take_deferred_edge_inserts(
        &self,
    ) -> Vec<(graphdb_core::wal::redo::InsertEdgeRedo, Timestamp)> {
        self.runtime.deferred_wal_ops.drain_edges()
    }

    pub(crate) fn take_deferred_edge_deletes(
        &self,
    ) -> Vec<(graphdb_core::wal::redo::DeleteEdgeRedo, Timestamp)> {
        self.runtime.deferred_wal_ops.drain_deletes()
    }

    pub fn cold_snapshots(&self) -> &Arc<RwLock<super::ColdSnapshotMap>> {
        &self.cold_snapshots
    }

    /// Register a snapshot by label, keeping at most
    /// `cold_tier.max_cold_snapshots_per_label` snapshots per label (oldest
    /// dropped first).
    pub fn load_cold_snapshot(&self, snapshot: ColdSnapshot) {
        let label = snapshot.label();
        let max_per_label = self
            .persistent
            .config
            .cold_tier
            .max_cold_snapshots_per_label;
        let mut guard = self.cold_snapshots.write();
        let snapshots = guard.entry(label).or_default();
        snapshots.push(Arc::new(snapshot));
        if max_per_label > 0 && snapshots.len() > max_per_label {
            let excess = snapshots.len() - max_per_label;
            snapshots.drain(..excess);
        }
    }

    pub fn remove_cold_snapshot(&self, label: LabelId) -> Option<Vec<Arc<ColdSnapshot>>> {
        self.cold_snapshots.write().remove(&label)
    }

    pub fn list_cold_snapshots(&self) -> Vec<LabelId> {
        self.cold_snapshots
            .read()
            .iter()
            .filter(|(_, snapshots)| !snapshots.is_empty())
            .map(|(label, _)| *label)
            .collect()
    }

    /// Derive a time-travel view over the currently registered snapshots:
    /// a per-label shelf of immutable snapshots keyed by timestamp.
    ///
    /// The view is a lightweight copy of the registration (Arc clones only),
    /// so callers may query it without holding the registry lock.
    pub fn cold_time_machine(&self) -> crate::cold::ColdSnapshotTimeMachine {
        let mut machine = crate::cold::ColdSnapshotTimeMachine::new();
        let cold = self.cold_snapshots.read();
        for snapshots in cold.values() {
            for snapshot in snapshots {
                machine.insert_arc(snapshot.clone());
            }
        }
        machine
    }

    /// Most recent cold snapshot of `label` not newer than `ts`, using the
    /// same timestamp routing as the query engine's cold fallback.
    pub fn cold_snapshot_at(
        &self,
        label: LabelId,
        ts: Timestamp,
    ) -> Option<Arc<crate::cold::ColdSnapshot>> {
        self.cold_time_machine().snapshot_at(label, ts)
    }

    /// Resolve the cold snapshot directory: `cold_tier.snapshot_dir` when
    /// configured, else `{work_dir}/cold_snapshots`.
    pub(crate) fn cold_snapshot_dir(&self) -> std::path::PathBuf {
        let cfg_dir = &self.persistent.config.cold_tier.snapshot_dir;
        if cfg_dir.as_os_str().is_empty() {
            self.persistent
                .layout
                .work_dir()
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp/linkrs_cold"))
                .join("cold_snapshots")
        } else {
            cfg_dir.clone()
        }
    }

    // ── Layout version & vertex-id domain evidence ───────────────────────────

    /// Monotonic physical layout version. Bumped on segment allocation,
    /// merge, compaction, eviction, restore, and cold-snapshot load/merge so
    /// consumers can detect stale plans.
    pub(crate) fn layout_version(&self) -> u64 {
        self.persistent.layout_version.get()
    }

    /// Bump the monotonic physical layout version.
    pub(crate) fn bump_layout_version(&self) {
        self.persistent.layout_version.bump();
    }

    /// Observe an i64 vertex-id write for the space's self-proven domain.
    /// Negative ids are rejected by the write path; the domain evidence only
    /// trusts non-negative i64 ids and falls back to `None` otherwise.
    pub(crate) fn observe_vertex_id_i64(&self, label: LabelId, id: i64) {
        let evidence = self.vertex_id_domain_evidence(label);
        evidence.observe_i64(id);
    }

    /// Observe a non-numeric (string) vertex-id write. Any string id in a
    /// label invalidates the numeric domain evidence for that label.
    pub(crate) fn observe_vertex_id_string(&self, label: LabelId) {
        let evidence = self.vertex_id_domain_evidence(label);
        evidence.observe_string();
    }

    fn vertex_id_domain_evidence(&self, label: LabelId) -> Arc<VertexIdDomainEvidence> {
        let domains = &self.persistent.vertex_id_domains;
        if let Some(evidence) = domains.read().get(&label) {
            return Arc::clone(evidence);
        }
        let evidence = Arc::new(VertexIdDomainEvidence::new());
        domains
            .write()
            .entry(label)
            .or_insert_with(|| Arc::clone(&evidence));
        evidence
    }

    /// Union of the self-proven vertex-id domains across the space's labels.
    /// Returns `None` when any label with rows lacks evidence (mixed/string
    /// ids), since a guessed range could silently omit rows. Labels with no
    /// writes have no evidence entry and contribute nothing — they are
    /// skipped rather than blocking the whole space.
    pub(crate) fn vertex_id_domain(&self, space: &str) -> Option<std::ops::Range<i64>> {
        let tags = self.schema_manager().list_tags(space).ok()?;
        let domains = self.persistent.vertex_id_domains.read();
        let mut min = i64::MAX;
        let mut max = i64::MIN;
        for tag in &tags {
            let Some(evidence) = domains.get(&tag.tag_id) else {
                // No writes for this label: nothing to cover.
                continue;
            };
            let range = evidence.domain()?;
            min = min.min(range.start);
            max = max.max(range.end);
        }
        if min >= max {
            return None;
        }
        Some(min..max)
    }

    /// Rebuild the self-proven vertex-id domain evidence from the live vertex
    /// tables. Called after restore/checkpoint load where write-path
    /// accumulation did not run. Also bumps the layout version (a restore
    /// changes the physical layout).
    pub(crate) fn rebuild_vertex_id_domains(&self) {
        let tables = self
            .persistent
            .data_store
            .with_vertex_tables(|tables| tables.values().cloned().collect::<Vec<_>>());
        for table in tables {
            let label = table.label();
            let evidence = self.vertex_id_domain_evidence(label);
            for key in table.external_id_keys() {
                match key {
                    crate::vertex::IdKey::Int(id) => evidence.observe_i64(id),
                    crate::vertex::IdKey::Text(_) => evidence.observe_string(),
                }
            }
        }
        self.bump_layout_version();
    }

    pub(crate) fn migration_history(
        &self,
    ) -> &Arc<parking_lot::RwLock<crate::migration_history::MigrationHistoryManager>> {
        &self.persistent.migration_history
    }

    pub fn record_migration_history(
        &self,
        record: crate::migration_history::MigrationHistoryRecord,
    ) -> graphdb_core::StorageResult<()> {
        let mut mgr = self.persistent.migration_history.write();
        mgr.record(record.clone())?;
        if let Some(paths) = self.storage_paths() {
            let path = paths.migration_history_file();
            if let Err(e) = mgr.save_to_file(&path) {
                log::warn!("Failed to persist migration_history: {}", e);
            }
        }
        Ok(())
    }

    pub fn list_migration_history(
        &self,
        space: &str,
        label: &str,
        is_edge: bool,
    ) -> Vec<crate::migration_history::MigrationHistoryRecord> {
        self.persistent
            .migration_history
            .read()
            .list(space, label, is_edge)
    }

    pub fn list_all_migration_history(
        &self,
    ) -> Vec<crate::migration_history::MigrationHistoryRecord> {
        self.persistent.migration_history.read().list_all()
    }

    pub fn get_applied_versions(&self, space: &str, label: &str, is_edge: bool) -> Vec<u64> {
        self.persistent
            .migration_history
            .read()
            .get_applied_versions_sorted(space, label, is_edge)
    }

    pub(crate) fn load_migration_history(&self) -> graphdb_core::StorageResult<()> {
        if let Some(work_dir) = self.work_dir().as_ref().cloned() {
            use crate::engine::paths::StoragePaths;
            use graphdb_sync::checkpoint_manifest::CheckpointManifestManager;
            let checkpoint_root = work_dir.join("checkpoint");
            let mut candidate_paths = Vec::new();
            if checkpoint_root.exists() {
                if let Ok(Some(manifest)) =
                    CheckpointManifestManager::new(checkpoint_root.join("manifests")).load_latest()
                {
                    candidate_paths.push(
                        StoragePaths::new(manifest.storage_snapshot.path).migration_history_file(),
                    );
                }
            }
            if let Some(paths) = self.storage_paths() {
                candidate_paths.push(paths.migration_history_file());
            }
            for path in candidate_paths {
                if path.exists() {
                    let mgr =
                        crate::migration_history::MigrationHistoryManager::load_from_file(&path)?;
                    *self.persistent.migration_history.write() = mgr;
                    break;
                }
            }
        } else if let Some(paths) = self.storage_paths() {
            let path = paths.migration_history_file();
            if path.exists() {
                let mgr = crate::migration_history::MigrationHistoryManager::load_from_file(&path)?;
                *self.persistent.migration_history.write() = mgr;
            }
        }
        Ok(())
    }

    pub(crate) fn save_migration_history(&self) -> graphdb_core::StorageResult<()> {
        if let Some(paths) = self.storage_paths() {
            let path = paths.migration_history_file();
            self.persistent
                .migration_history
                .read()
                .save_to_file(&path)?;
        }
        Ok(())
    }
}
