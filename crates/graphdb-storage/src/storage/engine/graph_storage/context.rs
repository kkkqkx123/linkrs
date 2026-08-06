use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::core::metadata::{IndexManager, SchemaManager};
use crate::core::stats::StatsManager;
use crate::core::types::{LabelId, TableTracker, TableTrackerConfig, Timestamp};
use crate::core::UserStorage;
use crate::core::{StorageError, StorageResult};
use crate::storage::cold::ColdSnapshot;
use crate::storage::engine::background_freeze::BackgroundFreezeManager;
use crate::storage::engine::cache_manager::CacheManager;
use crate::storage::engine::config::PropertyGraphConfig;
use crate::storage::engine::data_store::GraphDataStore;
use crate::storage::engine::paths::StoragePaths;
use crate::storage::engine::persistence_coordinator::PersistenceCoordinator;
use crate::storage::engine::resource_budget::{MemoryAccounting, MemoryBudget};
use crate::storage::engine::spiller::Spiller;
use crate::storage::index::{IndexDataManagerImpl, IndexGcConfig, IndexGcManager};
use crate::storage::mvcc::SnapshotHandle;
use crate::storage::vertex::{gc_manager::VertexGcManager, IdKey};
use crate::storage::StorageOperationContext;
use crate::transaction::VersionManager;
use graphdb_transaction::core::types::EdgeIdentifier;
use graphdb_transaction::transaction::undo_log::UndoLogManager;
use graphdb_transaction::transaction::{MutationResult, TransactionError, UndoLogEntry, VertexId};

type LastCompactedVertices = Arc<Mutex<Vec<(LabelId, Vec<IdKey>)>>>;
type CoreComponents = (
    Arc<GraphDataStore>,
    Arc<CacheManager>,
    Arc<TableTracker>,
    Arc<AtomicBool>,
    LastCompactedVertices,
    Arc<RwLock<IndexDataManagerImpl>>,
    Arc<SchemaManager>,
    Arc<IndexManager>,
    Arc<VersionManager>,
    Arc<UserStorage>,
    Arc<MemoryAccounting>,
);

#[derive(Clone)]
struct GraphStorageLayout {
    work_dir: Option<PathBuf>,
    db_path: String,
}

impl GraphStorageLayout {
    fn new() -> Self {
        Self {
            work_dir: None,
            db_path: String::new(),
        }
    }

    fn new_with_path(path: PathBuf) -> Self {
        Self {
            work_dir: Some(path.clone()),
            db_path: path.to_string_lossy().to_string(),
        }
    }

    fn work_dir(&self) -> &Option<PathBuf> {
        &self.work_dir
    }

    fn storage_paths(&self) -> Option<StoragePaths> {
        self.work_dir.as_ref().cloned().map(StoragePaths::new)
    }

    fn spill_dir(&self) -> PathBuf {
        self.work_dir
            .as_ref()
            .map(|p| p.join("spill"))
            .unwrap_or_else(|| PathBuf::from("/tmp/linkrs_spill"))
    }

    fn db_path(&self) -> &str {
        &self.db_path
    }
}

#[derive(Clone)]
struct GraphStoragePersistent {
    data_store: Arc<GraphDataStore>,
    cache_manager: Arc<CacheManager>,
    table_tracker: Arc<TableTracker>,
    config: PropertyGraphConfig,
    is_open: Arc<AtomicBool>,
    last_compacted_vertices: LastCompactedVertices,
    index_data_manager: Arc<RwLock<IndexDataManagerImpl>>,
    schema_manager: Arc<SchemaManager>,
    index_metadata_manager: Arc<IndexManager>,
    version_manager: Arc<VersionManager>,
    user_storage: Arc<UserStorage>,
    persistence: Option<Arc<RwLock<PersistenceCoordinator>>>,
    resource_accounting: Arc<MemoryAccounting>,
    spiller: Arc<Spiller>,
    layout: GraphStorageLayout,
    stats_manager: Option<Arc<StatsManager>>,
    next_auto_transaction_id: Arc<AtomicU64>,
    /// Serializes auto-commit DML statements (see [`AutoCommitWriteGate`]).
    auto_commit_write_gate: Arc<AutoCommitWriteGate>,
    staged_wal: Arc<
        dashmap::DashMap<
            crate::core::types::TransactionId,
            Vec<crate::transaction::wal::TransactionWalEntry>,
        >,
    >,
}

impl GraphStoragePersistent {
    fn dirty_flush_threshold(config: &PropertyGraphConfig) -> usize {
        match usize::try_from(config.resources.dirty_flush_operations) {
            Ok(value) => value,
            Err(_) => usize::MAX,
        }
    }

    fn build_core_components(
        config: &PropertyGraphConfig,
        index_root: Option<PathBuf>,
    ) -> CoreComponents {
        let resource_accounting = Self::new_resource_accounting(config);
        let cache_manager = Arc::new(CacheManager::new(
            config.enable_cache,
            config.cache_memory,
            &config.resources,
            resource_accounting.clone(),
        ));
        let table_tracker = Arc::new(TableTracker::with_config(TableTrackerConfig {
            flush_threshold: Self::dirty_flush_threshold(config),
            flush_interval: config.flush_config.flush_interval,
        }));

        (
            Arc::new(GraphDataStore::new()),
            cache_manager,
            table_tracker,
            Arc::new(AtomicBool::new(true)),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(RwLock::new(
                index_root.map_or_else(IndexDataManagerImpl::new, |root| {
                    IndexDataManagerImpl::new_with_root(root)
                }),
            )),
            Arc::new(SchemaManager::new()),
            Arc::new(IndexManager::new()),
            Arc::new(VersionManager::new()),
            Arc::new(UserStorage::new()),
            resource_accounting,
        )
    }

    pub fn new_with_config(config: PropertyGraphConfig) -> crate::core::StorageResult<Self> {
        config.validate()?;
        Ok(Self::from_validated_config(config))
    }

    fn from_validated_config(config: PropertyGraphConfig) -> Self {
        let resource_accounting = Self::new_resource_accounting(&config);
        let cache_manager = CacheManager::new(
            config.enable_cache,
            config.cache_memory,
            &config.resources,
            resource_accounting.clone(),
        );
        let table_tracker = Arc::new(TableTracker::with_config(TableTrackerConfig {
            flush_threshold: Self::dirty_flush_threshold(&config),
            flush_interval: config.flush_config.flush_interval,
        }));
        let data_store = Arc::new(GraphDataStore::new());
        let cache_manager = Arc::new(cache_manager);
        let spiller = Self::new_spiller(&config, &resource_accounting, &data_store, &cache_manager);

        Self {
            data_store,
            cache_manager,
            table_tracker,
            is_open: Arc::new(AtomicBool::new(true)),
            last_compacted_vertices: Arc::new(Mutex::new(Vec::new())),
            index_data_manager: {
                let dm = IndexDataManagerImpl::new();
                dm.set_memory_limit_bytes(config.resources.index_memory_bytes);
                dm.set_pool_capacity(config.resources.index_pool_capacity_bytes);
                dm.set_eviction_config(
                    config.resources.index_eviction_enabled,
                    config.resources.index_eviction_high_ratio,
                    config.resources.index_eviction_low_ratio,
                );
                Arc::new(RwLock::new(dm))
            },
            config,
            schema_manager: Arc::new(SchemaManager::new()),
            index_metadata_manager: Arc::new(IndexManager::new()),
            version_manager: Arc::new(VersionManager::new()),
            user_storage: Arc::new(UserStorage::new()),
            persistence: None,
            resource_accounting,
            spiller,
            layout: GraphStorageLayout::new(),
            stats_manager: None,
            // SQLite stores transaction IDs in signed INTEGER columns for the
            // durable outbox. Keep auto-generated IDs in the non-negative
            // i64 domain while leaving room below the high bit for callers.
            next_auto_transaction_id: Arc::new(AtomicU64::new(1 << 62)),
            auto_commit_write_gate: AutoCommitWriteGate::new(),
            staged_wal: Arc::new(dashmap::DashMap::new()),
        }
    }

    fn new() -> Self {
        Self::from_validated_config(PropertyGraphConfig::default())
    }

    fn new_resource_accounting(config: &PropertyGraphConfig) -> Arc<MemoryAccounting> {
        let resources = &config.resources;
        let budget = MemoryBudget::from_validated(
            resources.max_memory_bytes,
            resources.index_memory_bytes,
            resources.memory_soft_ratio,
            resources.memory_hard_ratio,
        );
        Arc::new(MemoryAccounting::new(budget))
    }

    fn new_spiller(
        config: &PropertyGraphConfig,
        accounting: &Arc<MemoryAccounting>,
        data_store: &Arc<GraphDataStore>,
        cache_manager: &Arc<CacheManager>,
    ) -> Arc<Spiller> {
        let spill_dir = GraphStorageLayout::new().spill_dir();
        Arc::new(Spiller::new(
            spill_dir,
            Arc::clone(accounting),
            Arc::clone(data_store),
            Arc::clone(cache_manager),
            config.resources.spill_threshold_ratio,
        ))
    }

    fn new_with_persistence(
        path: PathBuf,
        config: crate::storage::engine::PersistenceConfig,
    ) -> crate::core::StorageResult<Self> {
        let property_graph_config = config.property_graph_config.clone();
        property_graph_config.validate()?;
        let (
            data_store,
            cache_manager,
            table_tracker,
            is_open,
            last_compacted_vertices,
            index_data_manager,
            schema_manager,
            index_metadata_manager,
            version_manager,
            user_storage,
            resource_accounting,
        ) = Self::build_core_components(
            &property_graph_config,
            Some(StoragePaths::new(path.clone()).indexes_dir()),
        );

        let persistence = PersistenceCoordinator::new(config).map(|p| Arc::new(RwLock::new(p)))?;
        let barrier_registry = index_data_manager.read().barrier_registry();
        persistence
            .write()
            .set_index_barrier_registry(barrier_registry);

        let layout = GraphStorageLayout::new_with_path(path);
        let spill_dir = layout.spill_dir();
        let spiller = Arc::new(Spiller::new(
            spill_dir,
            Arc::clone(&resource_accounting),
            Arc::clone(&data_store),
            Arc::clone(&cache_manager),
            property_graph_config.resources.spill_threshold_ratio,
        ));

        Ok(Self {
            data_store,
            cache_manager,
            table_tracker,
            config: property_graph_config,
            is_open,
            last_compacted_vertices,
            index_data_manager,
            schema_manager,
            index_metadata_manager,
            version_manager,
            user_storage,
            persistence: Some(persistence),
            resource_accounting,
            spiller,
            layout,
            stats_manager: None,
            next_auto_transaction_id: Arc::new(AtomicU64::new(1 << 62)),
            auto_commit_write_gate: AutoCommitWriteGate::new(),
            staged_wal: Arc::new(dashmap::DashMap::new()),
        })
    }
}

#[derive(Clone)]
/// Deferred WAL operations for two-phase recovery.
/// Used to handle edge operations that depend on vertex existence.
struct DeferredWalOps {
    /// Deferred edge insertions (InsertEdgeRedo, Timestamp)
    edges: Arc<Mutex<Vec<(crate::core::wal::redo::InsertEdgeRedo, Timestamp)>>>,
    /// Deferred edge deletions (DeleteEdgeRedo, Timestamp)
    deletes: Arc<Mutex<Vec<(crate::core::wal::redo::DeleteEdgeRedo, Timestamp)>>>,
}

impl DeferredWalOps {
    fn new() -> Self {
        Self {
            edges: Arc::new(Mutex::new(Vec::new())),
            deletes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn push_edge(&self, edge: crate::core::wal::redo::InsertEdgeRedo, ts: Timestamp) {
        self.edges.lock().push((edge, ts));
    }

    fn push_delete(&self, delete: crate::core::wal::redo::DeleteEdgeRedo, ts: Timestamp) {
        self.deletes.lock().push((delete, ts));
    }

    fn drain_edges(&self) -> Vec<(crate::core::wal::redo::InsertEdgeRedo, Timestamp)> {
        self.edges.lock().drain(..).collect()
    }

    fn drain_deletes(&self) -> Vec<(crate::core::wal::redo::DeleteEdgeRedo, Timestamp)> {
        self.deletes.lock().drain(..).collect()
    }
}

#[derive(Clone)]
struct GraphStorageRuntime {
    index_gc_manager: Option<Arc<IndexGcManager>>,
    vertex_gc_manager: Option<Arc<VertexGcManager>>,
    background_freeze_manager: Option<Arc<BackgroundFreezeManager>>,
    deferred_wal_ops: DeferredWalOps,
    background_freeze_running: Arc<AtomicBool>,
    /// Last automatic vertex compaction time, for cooldown checks
    last_auto_compact: Arc<Mutex<Option<std::time::Instant>>>,
    /// Wall-clock time of the last write per edge label, for cold-tier idle checks
    last_edge_write: Arc<Mutex<HashMap<LabelId, std::time::Instant>>>,
    /// Last index-GC pass time, for throttling opportunistic GC (P5).
    last_index_gc: Arc<Mutex<Option<std::time::Instant>>>,
}

struct WriteTimestampLease {
    version_manager: Arc<VersionManager>,
    timestamp: Timestamp,
    finalized: AtomicBool,
}

impl Drop for WriteTimestampLease {
    fn drop(&mut self) {
        if !self.finalized.swap(true, Ordering::SeqCst) {
            self.version_manager.abort_write_timestamp(self.timestamp);
        }
    }
}

impl WriteTimestampLease {
    fn commit(&self) {
        if !self.finalized.swap(true, Ordering::SeqCst) {
            self.version_manager.commit_write_timestamp(self.timestamp);
        }
    }

    fn abort(&self) {
        if !self.finalized.swap(true, Ordering::SeqCst) {
            self.version_manager.abort_write_timestamp(self.timestamp);
        }
    }
}

/// Serializes auto-commit DML statements.
///
/// Auto-commit statements bypass the transaction manager, so they have no
/// write-set conflict detection. Serializing them with this gate gives
/// deterministic statement ordering: each auto-commit write observes every
/// previously committed auto-commit write, eliminating silent lost updates
/// (Last-Writer-Wins on read-modify-write). This mirrors the single-writer
/// default of other embedded graph databases. Read-only statements never
/// acquire the gate, so reads stay fully concurrent.
///
/// Implemented as a flag + condvar so the lease can be shared across clones
/// of the bound context; the lease is released on finalize or Drop.
struct AutoCommitWriteGate {
    locked: AtomicBool,
    mutex: Mutex<()>,
    condvar: parking_lot::Condvar,
}

impl AutoCommitWriteGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            locked: AtomicBool::new(false),
            mutex: Mutex::new(()),
            condvar: parking_lot::Condvar::new(),
        })
    }

    fn acquire(self: &Arc<Self>) -> Arc<AutoCommitWriteLease> {
        let mut guard = self.mutex.lock();
        while self.locked.swap(true, Ordering::Acquire) {
            self.condvar.wait(&mut guard);
        }
        drop(guard);
        Arc::new(AutoCommitWriteLease {
            gate: self.clone(),
            held: AtomicBool::new(true),
        })
    }

    fn release(&self) {
        self.locked.store(false, Ordering::Release);
        self.condvar.notify_one();
    }
}

struct AutoCommitWriteLease {
    gate: Arc<AutoCommitWriteGate>,
    held: AtomicBool,
}

impl AutoCommitWriteLease {
    /// Release the gate immediately (idempotent; Drop is the backstop).
    fn release(&self) {
        if self.held.swap(false, Ordering::AcqRel) {
            self.gate.release();
        }
    }
}

impl Drop for AutoCommitWriteLease {
    fn drop(&mut self) {
        if self.held.swap(false, Ordering::AcqRel) {
            self.gate.release();
        }
    }
}

/// Collects before-image undo entries while an auto-commit statement executes.
///
/// Auto-commit statements have no MVCC version chain (property updates are
/// in-place overwrites), so without undo a mid-statement failure would leave
/// the overwritten value physically present while the write timestamp is
/// marked Aborted. Recording undo entries lets `finalize_operation(false)`
/// roll the partial writes back, restoring the before-images.
struct AutoCommitMutationRecorder {
    undo: Arc<Mutex<UndoLogManager>>,
}

impl std::fmt::Debug for AutoCommitMutationRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoCommitMutationRecorder").finish()
    }
}

impl graphdb_transaction::transaction::TransactionMutationRecorder for AutoCommitMutationRecorder {
    fn record_mutation(&self, mutation: MutationResult) -> Result<(), TransactionError> {
        if let Some(entry) = mutation.undo_entry {
            self.add_undo_log(entry)
        } else {
            Ok(())
        }
    }

    fn record_vertex_write(&self, _vertex_id: VertexId) {}

    fn record_vertex_delete(&self, _vertex_id: VertexId) {}

    fn record_edge_write(&self, _edge: EdgeIdentifier) {}

    fn add_undo_log(&self, entry: UndoLogEntry) -> Result<(), TransactionError> {
        self.undo
            .lock()
            .add(entry)
            .map_err(|e| TransactionError::internal(e.to_string()))
    }

    fn record_table_modification(&self, _table_name: &str) {}
}

/// A shared auto-commit batch window (P4).
///
/// Acquires the auto-commit write gate and registers MVCC snapshots exactly
/// once for a run of auto-commit statements, so each statement inside the
/// window only allocates a fresh write timestamp, transaction id, and
/// before-image undo log instead of re-acquiring the gate and re-registering
/// every vertex/edge table's snapshots. Snapshot unregistration and gate
/// release happen in [`finalize`](Self::finalize) (or on `Drop` as a
/// backstop against abandoned windows).
///
/// Must be created from the pristine base context (no operation bound);
/// see [`GraphStorageContext::begin_auto_commit_batch`].
pub struct AutoCommitBatchWindow {
    base_ctx: Arc<GraphStorageContext>,
    /// Window-exclusive auto-commit write gate lease.
    gate_lease: Arc<AutoCommitWriteLease>,
    /// Write timestamp of the first statement; `None` until the first
    /// `bind_statement` call, which also registers the shared snapshots.
    first_ts: Mutex<Option<Timestamp>>,
    vertex_snapshot_handles: Mutex<Vec<(LabelId, SnapshotHandle)>>,
    edge_snapshot_registered: AtomicBool,
    /// Number of statements bound in this window (observation).
    statement_count: AtomicU64,
    /// Number of snapshot-registration rounds performed (observation; must be
    /// 1 for a correctly reused window).
    snapshot_rounds: AtomicU64,
}

impl AutoCommitBatchWindow {
    /// Bind one auto-commit statement inside this window.
    ///
    /// Allocates a fresh write timestamp (the first call also registers the
    /// shared MVCC snapshots at that timestamp), transaction id, and undo log.
    /// The returned context shares the window's snapshots and write gate and
    /// must be finalized via `finalize_operation` per statement.
    pub(crate) fn bind_statement(self: &Arc<Self>) -> StorageResult<GraphStorageContext> {
        let base = &self.base_ctx;
        let ts = {
            let mut first = self.first_ts.lock();
            match *first {
                Some(_) => base
                    .persistent
                    .version_manager
                    .try_next_write_timestamp()
                    .map_err(|error| StorageError::db_error(error.to_string()))?,
                None => {
                    let ts = base
                        .persistent
                        .version_manager
                        .try_next_write_timestamp()
                        .map_err(|error| StorageError::db_error(error.to_string()))?;
                    *first = Some(ts);
                    let (vertex_handles, edge_registered) =
                        base.register_auto_commit_snapshots(ts)?;
                    self.vertex_snapshot_handles.lock().extend(vertex_handles);
                    if edge_registered {
                        self.edge_snapshot_registered.store(true, Ordering::SeqCst);
                    }
                    self.snapshot_rounds.fetch_add(1, Ordering::SeqCst);
                    ts
                }
            }
        };

        let transaction_id = crate::core::types::TransactionId::new(
            base.persistent
                .next_auto_transaction_id
                .fetch_add(1, Ordering::SeqCst),
        );
        let undo_log = Arc::new(Mutex::new(UndoLogManager::new()));
        let context = StorageOperationContext {
            transaction_id: Some(transaction_id),
            read_timestamp: ts,
            write_timestamp: Some(ts),
            read_only: false,
            auto_commit: true,
            mutation_recorder: Some(Arc::new(AutoCommitMutationRecorder {
                undo: undo_log.clone(),
            })),
            mvcc_vertex_snapshot_handles: Vec::new(),
            mvcc_edge_snapshot_registered: false,
        };

        self.statement_count.fetch_add(1, Ordering::SeqCst);
        let mut bound = (**base).clone();
        bound.operation_context = Some(Arc::new(context));
        bound.write_timestamp_lease = Some(Arc::new(WriteTimestampLease {
            version_manager: base.persistent.version_manager.clone(),
            timestamp: ts,
            finalized: AtomicBool::new(false),
        }));
        bound.write_gate_lease = None;
        bound.auto_commit_undo = Some(undo_log);
        bound.auto_commit_window = Some(self.clone());
        Ok(bound)
    }

    /// Finalize the batch window: unregister the shared MVCC snapshots and
    /// release the write gate. Idempotent.
    pub fn finalize(&self) -> StorageResult<()> {
        self.unregister_snapshots();
        self.gate_lease.release();
        Ok(())
    }

    /// Number of statements bound in this window.
    pub fn statement_count(&self) -> u64 {
        self.statement_count.load(Ordering::SeqCst)
    }

    /// Number of snapshot-registration rounds performed (1 for a reused window).
    pub fn snapshot_rounds(&self) -> u64 {
        self.snapshot_rounds.load(Ordering::SeqCst)
    }

    /// Unregister all snapshots this window registered (idempotent).
    fn unregister_snapshots(&self) {
        let vertex_handles = std::mem::take(&mut *self.vertex_snapshot_handles.lock());
        if !vertex_handles.is_empty() {
            let tables = self
                .base_ctx
                .persistent
                .data_store
                .with_vertex_tables(|tables| {
                    vertex_handles
                        .iter()
                        .filter_map(|(label_id, _)| {
                            tables.get(label_id).map(|table| (*label_id, table.clone()))
                        })
                        .collect::<Vec<_>>()
                });
            for (label_id, vertex_table) in tables {
                for (handle_label, handle) in &vertex_handles {
                    if *handle_label == label_id {
                        let _ = vertex_table.unregister_snapshot(*handle);
                    }
                }
            }
        }

        if self.edge_snapshot_registered.swap(false, Ordering::SeqCst) {
            let first_ts = *self.first_ts.lock();
            if let Some(ts) = first_ts {
                let edge_tables = self
                    .base_ctx
                    .persistent
                    .data_store
                    .with_edge_tables(|tables| tables.values().cloned().collect::<Vec<_>>());
                for edge_table in edge_tables {
                    edge_table.write().unregister_snapshot(ts);
                }
            }
        }
    }
}

impl Drop for AutoCommitBatchWindow {
    fn drop(&mut self) {
        // Backstop for abandoned windows: unregister snapshots (idempotent);
        // the gate lease releases the write gate on Drop.
        self.unregister_snapshots();
    }
}

impl GraphStorageRuntime {
    fn new() -> Self {
        Self {
            index_gc_manager: None,
            vertex_gc_manager: None,
            background_freeze_manager: None,
            deferred_wal_ops: DeferredWalOps::new(),
            background_freeze_running: Arc::new(AtomicBool::new(false)),
            last_auto_compact: Arc::new(Mutex::new(None)),
            last_edge_write: Arc::new(Mutex::new(HashMap::new())),
            last_index_gc: Arc::new(Mutex::new(None)),
        }
    }

    fn with_index_gc(
        &self,
        index_data_manager: &Arc<RwLock<IndexDataManagerImpl>>,
        version_manager: &Arc<VersionManager>,
        config: IndexGcConfig,
    ) -> Self {
        let index_data = index_data_manager.read().clone();
        let gc_manager = IndexGcManager::new(index_data, version_manager.clone(), config);

        Self {
            index_gc_manager: Some(Arc::new(gc_manager)),
            vertex_gc_manager: self.vertex_gc_manager.clone(),
            background_freeze_manager: self.background_freeze_manager.clone(),
            deferred_wal_ops: self.deferred_wal_ops.clone(),
            background_freeze_running: self.background_freeze_running.clone(),
            last_auto_compact: self.last_auto_compact.clone(),
            last_edge_write: self.last_edge_write.clone(),
            last_index_gc: self.last_index_gc.clone(),
        }
    }

    fn with_vertex_gc(
        &self,
        data_store: &Arc<GraphDataStore>,
        version_manager: &Arc<VersionManager>,
        config: crate::storage::vertex::VertexGcConfig,
    ) -> Self {
        let gc_manager = VertexGcManager::new(data_store.clone(), version_manager.clone(), config);

        Self {
            index_gc_manager: self.index_gc_manager.clone(),
            vertex_gc_manager: Some(Arc::new(gc_manager)),
            background_freeze_manager: self.background_freeze_manager.clone(),
            deferred_wal_ops: self.deferred_wal_ops.clone(),
            background_freeze_running: self.background_freeze_running.clone(),
            last_auto_compact: self.last_auto_compact.clone(),
            last_edge_write: self.last_edge_write.clone(),
            last_index_gc: self.last_index_gc.clone(),
        }
    }

    fn with_background_freeze(&self, manager: Arc<BackgroundFreezeManager>) -> Self {
        Self {
            index_gc_manager: self.index_gc_manager.clone(),
            vertex_gc_manager: self.vertex_gc_manager.clone(),
            background_freeze_manager: Some(manager),
            deferred_wal_ops: self.deferred_wal_ops.clone(),
            background_freeze_running: self.background_freeze_running.clone(),
            last_auto_compact: self.last_auto_compact.clone(),
            last_edge_write: self.last_edge_write.clone(),
            last_index_gc: self.last_index_gc.clone(),
        }
    }

    fn start_index_gc(&self) -> Option<std::thread::JoinHandle<()>> {
        self.index_gc_manager
            .as_ref()
            .map(|gc: &Arc<IndexGcManager>| gc.start_background_gc())
    }

    fn stop_index_gc(&self) {
        if let Some(ref gc) = self.index_gc_manager {
            gc.stop();
        }
    }

    fn is_index_gc_running(&self) -> bool {
        self.index_gc_manager
            .as_ref()
            .map(|g: &Arc<IndexGcManager>| g.is_running())
            .unwrap_or(false)
    }

    /// P5: run an opportunistic index-GC pass, throttled to at most once every
    /// two seconds, so generation retirement/reclamation stays bounded even
    /// when no background GC thread is running.
    fn maybe_run_index_gc(&self) {
        let Some(gc) = self.index_gc_manager.as_ref() else {
            return;
        };
        {
            let mut last = self.last_index_gc.lock();
            if last
                .as_ref()
                .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(2))
            {
                return;
            }
            *last = Some(std::time::Instant::now());
        }
        let stats = gc.run_gc_pass();
        if !stats.is_empty() {
            log::debug!(
                "Opportunistic index GC removed {} entries",
                stats.total_removed()
            );
        }
    }

    fn start_vertex_gc(&self) -> Option<std::thread::JoinHandle<()>> {
        self.vertex_gc_manager
            .as_ref()
            .map(|gc: &Arc<VertexGcManager>| gc.start_background_gc())
    }

    fn stop_vertex_gc(&self) {
        if let Some(ref gc) = self.vertex_gc_manager {
            gc.stop();
        }
    }

    fn is_vertex_gc_running(&self) -> bool {
        self.vertex_gc_manager
            .as_ref()
            .map(|g: &Arc<VertexGcManager>| g.is_running())
            .unwrap_or(false)
    }
}

/// Per-edge-label list of registered cold snapshots, oldest first.
pub(crate) type ColdSnapshotMap = HashMap<LabelId, Vec<Arc<ColdSnapshot>>>;

#[derive(Clone)]
pub struct GraphStorageContext {
    persistent: GraphStoragePersistent,
    runtime: GraphStorageRuntime,
    operation_context: Option<Arc<StorageOperationContext>>,
    write_timestamp_lease: Option<Arc<WriteTimestampLease>>,
    /// Held while an auto-commit DML statement executes (see
    /// [`AutoCommitWriteGate`]); released on finalize or Drop.
    write_gate_lease: Option<Arc<AutoCommitWriteLease>>,
    /// Before-image undo log for the active auto-commit statement; applied on
    /// finalize(false) to roll back partial writes (see
    /// [`AutoCommitMutationRecorder`]).
    auto_commit_undo: Option<Arc<Mutex<UndoLogManager>>>,
    /// When `Some`, this context is bound inside an [`AutoCommitBatchWindow`]
    /// (P4): MVCC snapshots and the write-gate lease are owned by the window
    /// and shared across all statements of the batch. `finalize_operation`
    /// therefore skips per-statement snapshot unregistration and never
    /// releases the window's write gate.
    auto_commit_window: Option<Arc<AutoCommitBatchWindow>>,
    /// Read-only cold snapshots indexed by edge label ID, newest last.
    /// Loaded at startup from `.lkcs` files; hot-loaded at runtime via API.
    cold_snapshots: Arc<RwLock<ColdSnapshotMap>>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Module organization: split into logical groups
// ──────────────────────────────────────────────────────────────────────────────

mod accessors;
mod cache_index;
mod cold_tier;
mod edge_ops;
mod freeze;
pub(crate) mod helpers;
mod init;
mod maintenance;
mod persistence;
mod query;
mod schema;
mod vertex_ops;

pub use cache_index::ExportedEdgeSnapshotRecord;

impl std::fmt::Debug for GraphStorageContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphStorageContext").finish()
    }
}

impl GraphStorageContext {
    pub fn new_with_config(config: PropertyGraphConfig) -> crate::core::StorageResult<Self> {
        let persistent = GraphStoragePersistent::new_with_config(config)?;
        if let Err(e) = persistent.spiller.cleanup_stale_files() {
            log::warn!("Failed to clean up stale spill files: {}", e);
        }
        Ok(Self {
            persistent,
            runtime: GraphStorageRuntime::new(),
            operation_context: None,
            write_timestamp_lease: None,
            write_gate_lease: None,
            auto_commit_undo: None,
            auto_commit_window: None,
            cold_snapshots: Arc::new(RwLock::new(HashMap::new())),
        })
    }
}

impl Default for GraphStorageContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::thread;

    #[test]
    fn test_auto_commit_write_gate_mutual_exclusion() {
        let gate = AutoCommitWriteGate::new();
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let gate = gate.clone();
            let active = active.clone();
            let max_active = max_active.clone();
            handles.push(thread::spawn(move || {
                let _lease = gate.acquire();
                let cur = active.fetch_add(1, AtomicOrdering::SeqCst) + 1;
                max_active.fetch_max(cur, AtomicOrdering::SeqCst);
                thread::sleep(std::time::Duration::from_millis(5));
                active.fetch_sub(1, AtomicOrdering::SeqCst);
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(
            max_active.load(AtomicOrdering::SeqCst),
            1,
            "auto-commit write gate must never admit more than one writer"
        );
    }

    #[test]
    fn test_auto_commit_write_gate_release_is_idempotent() {
        let gate = AutoCommitWriteGate::new();
        let lease = gate.acquire();
        lease.release();
        // Releasing twice (release + Drop) must not panic or double-notify.
        drop(lease);
        // The gate must accept a new writer after release.
        let next = gate.acquire();
        drop(next);
    }
}
