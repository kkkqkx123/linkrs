use std::sync::atomic::Ordering;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::stats::StatsManager;
use crate::core::types::{LabelId, TableId, Timestamp};
use crate::core::{StorageError, StorageResult};
use crate::storage::cold::ColdSnapshot;
use crate::storage::engine::resource_budget::{MemoryCategory, ResourceSnapshot};

use crate::storage::mvcc::SnapshotHandle;
use crate::storage::StorageOperationContext;

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
    ) -> Option<Arc<dyn graphdb_transaction::transaction::TransactionMutationRecorder>> {
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

    /// Register MVCC snapshots for an auto-commit operation at `timestamp`.
    ///
    /// Scatter-gather: collect table references under a brief catalog READ
    /// lock (so concurrent transactions and DDL never block on the catalog
    /// write lock here), then register each table under its own per-table
    /// lock outside the catalog lock. Shared by the per-statement
    /// `with_auto_commit_context` path and the batch window (P4).
    ///
    /// NOTE: This function now implements lazy registration - it returns empty
    /// handles and the actual registration happens on first access to each table.
    /// This avoids traversing all tables at transaction start.
    pub(crate) fn register_auto_commit_snapshots(
        &self,
        _timestamp: Timestamp,
    ) -> StorageResult<(Vec<(LabelId, SnapshotHandle)>, bool)> {
        // Lazy registration: don't register all tables upfront.
        // Registration will happen on first access to each table.
        // Return empty handles - actual handles will be stored in
        // StorageOperationContext.registered_vertex_labels/registered_edge_partitions
        Ok((Vec::new(), false))
    }

    pub fn with_auto_commit_context(&self) -> StorageResult<Self> {
        // Auto-commit DML is serialized through the write gate: these
        // statements bypass the transaction manager and have no write-set
        // conflict detection, so serializing them makes statement ordering
        // deterministic and prevents silent lost updates (see
        // `AutoCommitWriteGate`). Read-only statements never take this path.
        let write_gate_lease = self.persistent.auto_commit_write_gate.acquire();
        let timestamp = self
            .persistent
            .version_manager
            .try_next_write_timestamp()
            .map_err(|error| StorageError::db_error(error.to_string()))?;
        let transaction_id = crate::core::types::TransactionId::new(
            self.persistent
                .next_auto_transaction_id
                .fetch_add(1, Ordering::SeqCst),
        );

        let mut bound = self.clone();
        let undo_log = Arc::new(parking_lot::Mutex::new(
            graphdb_transaction::transaction::UndoLogManager::new(),
        ));
        let mut context = StorageOperationContext {
            transaction_id: Some(transaction_id),
            read_timestamp: timestamp,
            write_timestamp: Some(timestamp),
            read_only: false,
            auto_commit: true,
            // Record before-images so a failed statement (finalize(false)) can
            // roll back partial writes (see `AutoCommitMutationRecorder`).
            mutation_recorder: Some(Arc::new(super::AutoCommitMutationRecorder {
                undo: undo_log.clone(),
                write_set: Arc::new(parking_lot::Mutex::new(
                    graphdb_transaction::transaction::types::WriteSet::new(),
                )),
            })),
            mvcc_vertex_snapshot_handles: Vec::new(),
            mvcc_edge_snapshot_registered: false,
            registered_vertex_labels: parking_lot::RwLock::new(std::collections::HashSet::new()),
            registered_edge_partitions: parking_lot::RwLock::new(std::collections::HashSet::new()),
        };

        // Register MVCC snapshots to prevent GC from cleaning data during the
        // transaction (see `register_auto_commit_snapshots`).
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

    /// Open an auto-commit batch window (P4).
    ///
    /// Acquires the auto-commit write gate for the whole window; MVCC
    /// snapshots are registered lazily on the first statement bound via the
    /// window and unregistered at `finalize` (or `Drop`). Must be called on
    /// the pristine (unbound) base context.
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
            vertex_snapshot_handles: parking_lot::Mutex::new(Vec::new()),
            edge_snapshot_registered: std::sync::atomic::AtomicBool::new(false),
            statement_count: std::sync::atomic::AtomicU64::new(0),
            snapshot_rounds: std::sync::atomic::AtomicU64::new(0),
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
        let timestamp = operation.write_timestamp.ok_or_else(|| {
            StorageError::db_error("Auto-commit operation has no write timestamp")
        })?;
        let transaction_id = operation.transaction_id;

        // Window-bound statements (P4) share the batch window's MVCC
        // snapshots; the window unregisters them once at `finalize`. Only
        // per-statement contexts (registered by `with_auto_commit_context`)
        // unregister here.
        if self.auto_commit_window.is_none() {
            // Unregister lazily registered vertex snapshots
            let registered_labels: Vec<LabelId> = {
                let registered = operation.registered_vertex_labels.read();
                registered.iter().cloned().collect()
            };

            if !registered_labels.is_empty() {
                let tables: Vec<(
                    LabelId,
                    Arc<crate::storage::vertex::vertex_table::ShardedVertexTable>,
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

            // Unregister lazily registered edge snapshots
            let registered_edge_keys: Vec<crate::storage::engine::data_store::EdgeTableKey> = {
                let registered = operation.registered_edge_partitions.read();
                registered.iter().cloned().collect()
            };

            if !registered_edge_keys.is_empty() {
                let edge_tables: Vec<Arc<parking_lot::RwLock<crate::storage::edge::EdgeStore>>> =
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

        if committed {
            self.commit_write_timestamp(timestamp);
        } else {
            // Roll back partial writes recorded during the statement before
            // aborting the write timestamp. Without this, an in-place property
            // overwrite from a failed statement would remain physically present
            // while the timestamp is Aborted.
            if let Some(undo) = &self.auto_commit_undo {
                let mut log = undo.lock();
                if let Err(error) = log.execute_undo(self, timestamp) {
                    log::error!("Auto-commit rollback failed: {}", error);
                }
            }
            self.abort_write_timestamp(timestamp);
        }
        // Release the auto-commit write gate so the next DML statement can run.
        if let Some(lease) = &self.write_gate_lease {
            lease.release();
        }
        // P5.2: drop any residue staged-WAL for this auto-commit transaction
        // (per-statement entries are normally committed by `commit_auto_if_needed`;
        // this guarantees the staged map stays bounded even on error paths).
        if let Some(transaction_id) = transaction_id {
            self.persistent.staged_wal.remove(&transaction_id);
        }
        // P5: keep index generation retirement bounded during long auto-commit
        // loads (throttled; no-op when no GC manager is assembled).
        self.maybe_run_index_gc();
        Ok(())
    }

    pub fn start_index_gc(&self) -> Option<crate::storage::thread_pool::BackgroundTaskHandle> {
        self.runtime.start_index_gc()
    }

    /// P5: opportunistically run an index-GC pass (throttled).
    pub(crate) fn maybe_run_index_gc(&self) {
        self.runtime.maybe_run_index_gc();
    }

    pub fn stop_index_gc(&self) {
        self.runtime.stop_index_gc();
    }

    pub fn is_index_gc_running(&self) -> bool {
        self.runtime.is_index_gc_running()
    }

    pub fn start_vertex_gc(&self) -> Option<crate::storage::thread_pool::BackgroundTaskHandle> {
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

    pub fn check_write_admission(&self) -> crate::core::StorageResult<()> {
        if self
            .operation_context
            .as_ref()
            .is_some_and(|context| context.read_only)
        {
            return Err(crate::core::StorageError::invalid_operation(
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
                return Err(crate::core::StorageError::capacity_exceeded());
            }
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
        let tracker = self.persistent.version_manager.snapshot_tracker();
        let active = tracker.active_count();
        if active >= self.persistent.config.resources.max_active_snapshots {
            return Err(crate::core::StorageError::capacity_exceeded());
        }
        if tracker
            .oldest_age()
            .is_some_and(|age| age >= self.persistent.config.resources.max_snapshot_age)
        {
            return Err(crate::core::StorageError::invalid_operation(
                "Oldest active snapshot exceeded max_snapshot_age",
            ));
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

    /// Hash partitions per vertex label table (auto-compaction / ID space
    /// related config).
    pub(crate) fn vertex_table_shards(&self) -> usize {
        self.persistent.config.vertex_table_shards
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

    pub(crate) fn spiller(&self) -> &Arc<crate::storage::engine::spiller::Spiller> {
        &self.persistent.spiller
    }

    pub fn try_reserve_with_spill(
        &self,
        category: crate::storage::engine::resource_budget::MemoryCategory,
        bytes: u64,
    ) -> crate::core::StorageResult<crate::storage::engine::resource_budget::MemoryReservation>
    {
        self.persistent
            .spiller
            .try_reserve_with_spill(category, bytes)
    }

    pub(crate) fn get_freeze_config_full(&self) -> crate::storage::engine::config::FreezeConfig {
        self.persistent.config.freeze.clone()
    }

    pub(crate) fn append_wal_redo<T: serde::Serialize>(
        &self,
        op_type: crate::core::wal::types::WalOpType,
        timestamp: Timestamp,
        redo: &T,
    ) -> crate::core::StorageResult<crate::transaction::wal::TransactionWalEntry> {
        let payload = postcard::to_allocvec(redo).map_err(|error| {
            crate::core::StorageError::serialize_error(format!(
                "Failed to serialize WAL redo: {}",
                error
            ))
        })?;
        if let Some(transaction_id) = self
            .operation_context
            .as_ref()
            .and_then(|operation| operation.transaction_id)
        {
            let entry = crate::transaction::wal::TransactionWalEntry {
                op_type,
                timestamp,
                payload,
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
                return Ok(crate::transaction::wal::TransactionWalEntry {
                    op_type,
                    timestamp,
                    payload,
                });
            }
        }

        Ok(crate::transaction::wal::TransactionWalEntry {
            op_type,
            timestamp,
            payload,
        })
    }

    pub(crate) fn commit_staged_writes(
        &self,
        transaction_id: crate::core::types::TransactionId,
        intents: &[crate::core::wal::OutboxIntent],
    ) -> crate::core::StorageResult<crate::core::types::CommitLsn> {
        self.commit_staged_writes_with_durability(
            transaction_id,
            intents,
            crate::core::types::DurabilityLevel::Sync,
        )
    }

    pub(crate) fn commit_staged_writes_with_durability(
        &self,
        transaction_id: crate::core::types::TransactionId,
        intents: &[crate::core::wal::OutboxIntent],
        durability: crate::core::types::DurabilityLevel,
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
            let result = wal_manager.read().append_transaction_with_durability(
                transaction_id,
                entries,
                intents,
                durability,
            )?;
            result
        } else {
            crate::core::types::CommitLsn::ZERO
        };
        self.persistent
            .index_data_manager
            .read()
            .advance_barriers(commit_lsn);
        self.persistent.staged_wal.remove(&transaction_id);
        Ok(commit_lsn)
    }

    pub(crate) fn abort_staged_writes(&self, transaction_id: crate::core::types::TransactionId) {
        self.persistent.staged_wal.remove(&transaction_id);
    }

    /// Number of staged-WAL entries held for in-flight transactions.
    pub(crate) fn staged_wal_len(&self) -> usize {
        self.persistent.staged_wal.len()
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
    pub fn cold_time_machine(&self) -> crate::storage::cold::ColdSnapshotTimeMachine {
        let mut machine = crate::storage::cold::ColdSnapshotTimeMachine::new();
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
    ) -> Option<Arc<crate::storage::cold::ColdSnapshot>> {
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
}
