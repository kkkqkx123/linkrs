use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
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
use crate::storage::engine::data_store::{EdgeTableKey, GraphDataStore};
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
use graphdb_transaction::transaction::{
    MutationEntityKey, MutationResult, TransactionError, UndoLogEntry, VertexId,
};

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

/// Monotonic counter for the physical vertex/edge layout.
///
/// Bumped whenever segment allocation, merge, compaction, eviction, or
/// restore changes the on-disk/in-memory layout of vertex or edge tables.
/// Consumers (e.g. the query plan cache) compare this version to detect
/// stale plans that assumed an older layout.
pub(crate) struct LayoutVersion {
    value: Arc<AtomicU64>,
}

impl LayoutVersion {
    fn new() -> Self {
        Self {
            value: Arc::new(AtomicU64::new(1)),
        }
    }

    pub(crate) fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    pub(crate) fn bump(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }
}

impl Clone for LayoutVersion {
    fn clone(&self) -> Self {
        Self {
            value: Arc::clone(&self.value),
        }
    }
}

impl std::fmt::Debug for LayoutVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutVersion")
            .field("value", &self.get())
            .finish()
    }
}

/// Per-label vertex-id domain evidence.
///
/// The partition planner requires a vertex-id range that provably covers the
/// scanned domain; guessing a range can silently omit rows. This evidence is
/// accumulated on every write (and rebuilt after restore) so the storage can
/// self-prove a covering `[min, max]` range when *all* vertex ids of the
/// label are numeric and non-negative.
#[derive(Debug)]
pub(crate) struct VertexIdDomainEvidence {
    min_id: AtomicI64,
    max_id: AtomicI64,
    saw_string_id: AtomicBool,
}

impl VertexIdDomainEvidence {
    fn new() -> Self {
        Self {
            min_id: AtomicI64::new(i64::MAX),
            max_id: AtomicI64::new(i64::MIN),
            saw_string_id: AtomicBool::new(false),
        }
    }

    fn observe_i64(&self, id: i64) {
        self.min_id.fetch_min(id, Ordering::Relaxed);
        self.max_id.fetch_max(id, Ordering::Relaxed);
    }

    fn observe_string(&self) {
        self.saw_string_id.store(true, Ordering::Relaxed);
    }

    fn domain(&self) -> Option<std::ops::Range<i64>> {
        if self.saw_string_id.load(Ordering::Relaxed) {
            return None;
        }
        let min = self.min_id.load(Ordering::Relaxed);
        let max = self.max_id.load(Ordering::Relaxed);
        if min > max {
            return None;
        }
        Some(min..max.saturating_add(1))
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
    /// Monotonic physical layout version for stale-plan detection.
    layout_version: LayoutVersion,
    /// Self-proven vertex-id domains keyed by label (see
    /// [`VertexIdDomainEvidence`]).
    vertex_id_domains: Arc<RwLock<std::collections::HashMap<LabelId, Arc<VertexIdDomainEvidence>>>>,
    /// SERIAL column allocators (one counter per space + table).
    serial_allocator: crate::storage::engine::graph_storage::serial::SerialAllocator,
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
            layout_version: LayoutVersion::new(),
            vertex_id_domains: Arc::new(RwLock::new(std::collections::HashMap::new())),
            serial_allocator: crate::storage::engine::graph_storage::serial::SerialAllocator::new(),
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
            layout_version: LayoutVersion::new(),
            vertex_id_domains: Arc::new(RwLock::new(std::collections::HashMap::new())),
            serial_allocator: crate::storage::engine::graph_storage::serial::SerialAllocator::new(),
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
    /// Shared rayon-backed pool for background maintenance work (freeze,
    /// vertex/index GC loops). See [`crate::storage::thread_pool`].
    thread_pool: Arc<crate::storage::thread_pool::StorageThreadPool>,
    background_freeze_running: Arc<AtomicBool>,
    /// Last automatic vertex compaction time, for cooldown checks
    last_auto_compact: Arc<Mutex<Option<std::time::Instant>>>,
    /// Wall-clock time of the last write per edge label, for cold-tier idle checks
    last_edge_write: Arc<Mutex<HashMap<LabelId, std::time::Instant>>>,
    /// Last index-GC pass time, for throttling opportunistic GC.
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

/// Cumulative gate admission statistics (acquisitions and total wait time).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WriteGateStats {
    /// Number of gate acquisitions (statements serialized).
    pub acquisitions: u64,
    /// Total time spent waiting for admission, in nanoseconds.
    pub wait_nanos: u64,
}

/// Serializes auto-commit DML statements.
struct AutoCommitWriteGate {
    /// Gate state, guarded by `mutex`: the holder thread and its lease depth.
    ///
    /// The gate is **re-entrant per thread**: a statement-level auto-commit
    /// binding holds the gate while the statement executes, and nested gated
    /// operations on the same thread (e.g. `COPY FROM` opening its group
    /// window inside that statement) must not self-deadlock. Nested leases
    /// only bump the depth; the gate frees when the outermost lease releases.
    state: Mutex<Option<(std::thread::ThreadId, u64)>>,
    condvar: parking_lot::Condvar,
    /// Cumulative admission counters (see [`WriteGateStats`]).
    acquisitions: AtomicU64,
    wait_nanos: AtomicU64,
}

impl AutoCommitWriteGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(None),
            condvar: parking_lot::Condvar::new(),
            acquisitions: AtomicU64::new(0),
            wait_nanos: AtomicU64::new(0),
        })
    }

    fn acquire(self: &Arc<Self>) -> Arc<AutoCommitWriteLease> {
        let start = std::time::Instant::now();
        let current = std::thread::current().id();
        let mut guard = self.state.lock();
        // Re-entrant admission: the holder re-acquiring bumps the depth.
        if let Some((holder, depth)) = guard.as_mut() {
            if *holder == current {
                *depth += 1;
                drop(guard);
                self.acquisitions.fetch_add(1, Ordering::Relaxed);
                return Arc::new(AutoCommitWriteLease {
                    gate: self.clone(),
                    held: AtomicBool::new(true),
                });
            }
        }
        while guard.is_some() {
            self.condvar.wait(&mut guard);
        }
        *guard = Some((current, 1));
        drop(guard);
        self.acquisitions.fetch_add(1, Ordering::Relaxed);
        self.wait_nanos
            .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
        Arc::new(AutoCommitWriteLease {
            gate: self.clone(),
            held: AtomicBool::new(true),
        })
    }

    fn stats(&self) -> WriteGateStats {
        WriteGateStats {
            acquisitions: self.acquisitions.load(Ordering::Relaxed),
            wait_nanos: self.wait_nanos.load(Ordering::Relaxed),
        }
    }

    fn release(&self) {
        let mut guard = self.state.lock();
        match guard.as_mut() {
            Some((_, depth)) if *depth > 1 => *depth -= 1,
            Some(_) => {
                *guard = None;
                drop(guard);
                self.condvar.notify_one();
            }
            None => {}
        }
    }
}

struct AutoCommitWriteLease {
    gate: Arc<AutoCommitWriteGate>,
    held: AtomicBool,
}

impl AutoCommitWriteLease {
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

/// Collects before-image undo entries and write-set entity keys while an
/// auto-commit statement executes.
struct AutoCommitMutationRecorder {
    undo: Arc<Mutex<UndoLogManager>>,
    /// Write set for conflict detection (tracks modified vertices and edges)
    write_set: Arc<Mutex<graphdb_transaction::transaction::types::WriteSet>>,
}

impl std::fmt::Debug for AutoCommitMutationRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoCommitMutationRecorder").finish()
    }
}

impl graphdb_transaction::transaction::TransactionMutationRecorder for AutoCommitMutationRecorder {
    fn record_mutation(&self, mutation: MutationResult) -> Result<(), TransactionError> {
        for entity_key in mutation.entity_keys {
            match entity_key {
                MutationEntityKey::Vertex(vertex_id) => self.record_vertex_write(vertex_id),
                MutationEntityKey::Edge(edge) => self.record_edge_write(edge),
            }
        }
        if let Some(entry) = mutation.undo_entry {
            self.add_undo_log(entry)
        } else {
            Ok(())
        }
    }

    fn record_vertex_write(&self, vertex_id: VertexId) {
        self.write_set.lock().record_vertex(vertex_id);
    }

    fn record_vertex_delete(&self, vertex_id: VertexId) {
        self.write_set.lock().record_vertex_delete(vertex_id);
    }

    fn record_edge_write(&self, edge: EdgeIdentifier) {
        self.write_set.lock().record_edge(edge);
    }

    fn add_undo_log(&self, entry: UndoLogEntry) -> Result<(), TransactionError> {
        self.undo
            .lock()
            .add(entry)
            .map_err(|e| TransactionError::internal(e.to_string()))
    }

    fn record_table_modification(&self, _table_name: &str) {}
}

/// A shared auto-commit batch window.
pub struct AutoCommitBatchWindow {
    base_ctx: Arc<GraphStorageContext>,
    gate_lease: Arc<AutoCommitWriteLease>,
    first_ts: Mutex<Option<Timestamp>>,
    /// Lazily registered vertex snapshots as (label, handle) pairs. One
    /// entry per registration; the per-timestamp refcount stays balanced when
    /// the same label is registered by several statements (group mode shares
    /// one timestamp).
    registered_vertex_snapshots: Mutex<Vec<(LabelId, SnapshotHandle)>>,
    /// Lazily registered edge-partition snapshots as (key, timestamp) pairs.
    registered_edge_snapshots: Mutex<Vec<(EdgeTableKey, Timestamp)>>,
    statement_count: AtomicU64,
    snapshot_rounds: AtomicU64,
    /// Group mode: statements share one write timestamp (first_ts),
    /// one undo log, and one commit point; see `begin_auto_commit_group`.
    group: AtomicBool,
    /// Shared before-image undo log for group mode (one segment per statement).
    group_undo: Option<Arc<Mutex<UndoLogManager>>>,
}

impl AutoCommitBatchWindow {
    pub(crate) fn bind_statement(self: &Arc<Self>) -> StorageResult<GraphStorageContext> {
        let base = &self.base_ctx;
        let is_group = self.group.load(Ordering::Acquire);
        let ts = {
            let mut first = self.first_ts.lock();
            match *first {
                Some(ts) if is_group => ts,
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
        let undo_log = if is_group {
            Arc::clone(
                self.group_undo
                    .as_ref()
                    .expect("group mode requires group_undo to be initialized"),
            )
        } else {
            Arc::new(Mutex::new(UndoLogManager::new()))
        };
        let group_undo_start = if is_group {
            Some(undo_log.lock().len())
        } else {
            None
        };
        let context = StorageOperationContext {
            transaction_id: Some(transaction_id),
            read_timestamp: ts,
            write_timestamp: Some(ts),
            read_only: false,
            auto_commit: true,
            mutation_recorder: Some(Arc::new(AutoCommitMutationRecorder {
                undo: undo_log.clone(),
                write_set: Arc::new(parking_lot::Mutex::new(
                    graphdb_transaction::transaction::types::WriteSet::new(),
                )),
            })),
            mvcc_vertex_snapshot_handles: Vec::new(),
            mvcc_edge_snapshot_registered: false,
            registered_vertex_labels: parking_lot::RwLock::new(std::collections::HashSet::new()),
            registered_edge_partitions: parking_lot::RwLock::new(std::collections::HashSet::new()),
            auto_commit_group_start: group_undo_start,
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

    pub fn finalize(&self) -> StorageResult<()> {
        self.unregister_snapshots();
        self.gate_lease.release();
        Ok(())
    }

    pub fn statement_count(&self) -> u64 {
        self.statement_count.load(Ordering::SeqCst)
    }

    pub fn snapshot_rounds(&self) -> u64 {
        self.snapshot_rounds.load(Ordering::SeqCst)
    }

    pub fn is_grouped(&self) -> bool {
        self.group.load(Ordering::Acquire)
    }

    /// Single group commit point: one fsync, then barrier advance, then the
    /// shared write-timestamp commit. Order: durability → visibility.
    pub fn finalize_group(&self) -> StorageResult<()> {
        // 1) Durability: one sync covering every no-wait appended statement.
        if let Some(persistence) = self.base_ctx.persistent.persistence.as_ref() {
            if let Some(wal) = persistence.read().wal_manager() {
                wal.read().sync()?;
                let durable = wal.read().durable_lsn();
                self.base_ctx
                    .persistent
                    .index_data_manager
                    .read()
                    .advance_barriers(crate::core::types::CommitLsn::new(durable.as_u64()));
            }
        }
        // 2) Visibility: commit the shared write timestamp once.
        if let Some(ts) = *self.first_ts.lock() {
            self.base_ctx
                .persistent
                .version_manager
                .commit_write_timestamp(ts);
        }
        // 3) Window cleanup (unchanged): unregister snapshots, release gate.
        self.unregister_snapshots();
        self.gate_lease.release();
        Ok(())
    }

    /// Roll back every statement bound to this group window: execute the
    /// shared undo log against the base context, abort the shared write
    /// timestamp, and release snapshots and the write gate.
    ///
    /// Only meaningful in group mode; batch windows have per-statement undo
    /// logs owned by their bound statements.
    pub fn rollback_group(&self) -> StorageResult<()> {
        if let Some(undo) = &self.group_undo {
            let mut manager = undo.lock();
            let start_ts = self.first_ts.lock().unwrap_or(0);
            manager
                .execute_undo(&*self.base_ctx, start_ts)
                .map_err(|e| StorageError::db_error(e.to_string()))?;
            drop(manager);
            if let Some(ts) = *self.first_ts.lock() {
                self.base_ctx.abort_write_timestamp(ts);
            }
        }
        self.unregister_snapshots();
        self.gate_lease.release();
        Ok(())
    }

    fn unregister_snapshots(&self) {
        // Unregister every vertex snapshot registered lazily by the window's
        // statements. Each entry matches one registration (handle), so the
        // per-timestamp refcounts return to zero exactly.
        let vertex_registrations = std::mem::take(&mut *self.registered_vertex_snapshots.lock());
        if !vertex_registrations.is_empty() {
            let tables = self
                .base_ctx
                .persistent
                .data_store
                .with_vertex_tables(|tables| tables.values().cloned().collect::<Vec<_>>());
            for (label_id, handle) in vertex_registrations {
                for table in &tables {
                    if table.label() == label_id {
                        let _ = table.unregister_snapshot(handle);
                        break;
                    }
                }
            }
        }

        let edge_registrations = std::mem::take(&mut *self.registered_edge_snapshots.lock());
        if !edge_registrations.is_empty() {
            let edge_tables = self
                .base_ctx
                .persistent
                .data_store
                .with_edge_tables(|tables| tables.clone());
            for (key, ts) in edge_registrations {
                if let Some(edge_table) = edge_tables.get(&key) {
                    edge_table.write().unregister_snapshot(ts);
                }
            }
        }
    }
}

impl Drop for AutoCommitBatchWindow {
    fn drop(&mut self) {
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
            thread_pool: Arc::new(
                crate::storage::thread_pool::StorageThreadPool::new().unwrap_or_else(|e| {
                    log::error!("Failed to build storage thread pool: {}", e);
                    crate::storage::thread_pool::StorageThreadPool::default()
                }),
            ),
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
        let gc_manager = IndexGcManager::new(
            index_data,
            version_manager.clone(),
            config,
            self.thread_pool.clone(),
        );

        Self {
            index_gc_manager: Some(Arc::new(gc_manager)),
            vertex_gc_manager: self.vertex_gc_manager.clone(),
            background_freeze_manager: self.background_freeze_manager.clone(),
            deferred_wal_ops: self.deferred_wal_ops.clone(),
            thread_pool: self.thread_pool.clone(),
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
        let gc_manager = VertexGcManager::new(
            data_store.clone(),
            version_manager.clone(),
            config,
            self.thread_pool.clone(),
        );

        Self {
            index_gc_manager: self.index_gc_manager.clone(),
            vertex_gc_manager: Some(Arc::new(gc_manager)),
            background_freeze_manager: self.background_freeze_manager.clone(),
            deferred_wal_ops: self.deferred_wal_ops.clone(),
            thread_pool: self.thread_pool.clone(),
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
            thread_pool: self.thread_pool.clone(),
            background_freeze_running: self.background_freeze_running.clone(),
            last_auto_compact: self.last_auto_compact.clone(),
            last_edge_write: self.last_edge_write.clone(),
            last_index_gc: self.last_index_gc.clone(),
        }
    }

    fn start_index_gc(&self) -> Option<crate::storage::thread_pool::BackgroundTaskHandle> {
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

    fn start_vertex_gc(&self) -> Option<crate::storage::thread_pool::BackgroundTaskHandle> {
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
    /// MVCC snapshots and the write-gate lease are owned by the window
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

    /// Cumulative auto-commit write-gate admission statistics. Used by
    /// benchmarks (B2) to attribute write-path time to gate contention.
    pub fn write_gate_stats(&self) -> WriteGateStats {
        self.persistent.auto_commit_write_gate.stats()
    }

    /// Lazily register a vertex label snapshot if not already registered.
    /// Returns the snapshot handle if registration succeeded.
    ///
    /// Read-only statement contexts (see `with_read_operation_context`) pin
    /// their read timestamp the same way auto-commit writes pin their write
    /// timestamp, so a long-running read cannot be unterminated by GC while
    /// the statement still observes the snapshot.
    pub(crate) fn ensure_vertex_snapshot_registered(
        &self,
        label: LabelId,
    ) -> Option<crate::storage::mvcc::SnapshotHandle> {
        let operation = self.operation_context.as_ref()?;
        if !operation.auto_commit {
            return None;
        }

        // Check if already registered (using read lock)
        {
            let registered = operation.registered_vertex_labels.read();
            if registered.contains(&label) {
                return None;
            }
        }

        // Register snapshot for this label
        let timestamp = operation.snapshot_timestamp()?;
        let vertex_tables = self
            .persistent
            .data_store
            .with_vertex_tables(|tables| tables.get(&label).cloned())?;

        let handle = vertex_tables.register_snapshot(timestamp).ok()?;

        // Store the label in the registered set (using write lock)
        {
            let mut registered = operation.registered_vertex_labels.write();
            registered.insert(label);
        }

        // Batch windows own the snapshot lifecycle: record the registration so
        // the window can unregister it (once per entry) at finalize. Without
        // this the lazily registered snapshot would pin the table's GC
        // watermark forever.
        if let Some(window) = &self.auto_commit_window {
            window
                .registered_vertex_snapshots
                .lock()
                .push((label, handle));
        }

        Some(handle)
    }

    /// Lazily register an edge partition snapshot if not already registered.
    ///
    /// Supports both auto-commit write contexts (write timestamp) and
    /// read-only statement contexts (read timestamp).
    pub(crate) fn ensure_edge_snapshot_registered(
        &self,
        edge_key: crate::storage::engine::data_store::EdgeTableKey,
    ) -> bool {
        let operation = match self.operation_context.as_ref() {
            Some(op) if op.auto_commit => op,
            _ => return false,
        };

        // Check if already registered (using read lock)
        {
            let registered = operation.registered_edge_partitions.read();
            if registered.contains(&edge_key) {
                return true;
            }
        }

        // Register snapshot for this edge partition
        let Some(timestamp) = operation.snapshot_timestamp() else {
            return false;
        };

        let edge_tables = self
            .persistent
            .data_store
            .with_edge_tables(|tables| tables.get(&edge_key).cloned());

        if let Some(edge_table) = edge_tables {
            edge_table.write().register_snapshot(timestamp);

            // Store the edge key in the registered set (using write lock)
            {
                let mut registered = operation.registered_edge_partitions.write();
                registered.insert(edge_key);
            }

            // Batch windows own the snapshot lifecycle: record the
            // registration for window-level unregistration at finalize.
            if let Some(window) = &self.auto_commit_window {
                window
                    .registered_edge_snapshots
                    .lock()
                    .push((edge_key, timestamp));
            }

            true
        } else {
            false
        }
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

    use graphdb_transaction::transaction::TransactionMutationRecorder;

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

    #[test]
    fn test_auto_commit_write_gate_is_reentrant_per_thread() {
        // Regression: a statement-level auto-commit binding holds the gate
        // while a nested gated operation on the same thread (COPY FROM group
        // window) acquires it again. Non-reentrant admission self-deadlocked.
        let gate = AutoCommitWriteGate::new();
        let outer = gate.acquire();
        {
            // Same-thread re-acquisition must not block.
            let inner = gate.acquire();
            let innermost = gate.acquire();
            drop(inner);
            drop(innermost);
        }
        // Depth still 1 after the nested leases dropped: another thread must
        // stay excluded until `outer` releases.
        let spawned = thread::spawn({
            let gate = gate.clone();
            move || {
                let lease = gate.acquire();
                drop(lease);
                true
            }
        });
        thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            !spawned.is_finished(),
            "gate admitted a second holder while leased"
        );
        drop(outer);
        assert!(
            spawned.join().unwrap(),
            "waiter must acquire after outer release"
        );
    }

    #[test]
    fn test_auto_commit_recorder_forwards_entity_keys_to_write_set() {
        let recorder = AutoCommitMutationRecorder {
            undo: Arc::new(Mutex::new(UndoLogManager::new())),
            write_set: Arc::new(Mutex::new(
                graphdb_transaction::transaction::types::WriteSet::new(),
            )),
        };

        let vertex_id = VertexId::from_int64(42);
        let edge = EdgeIdentifier {
            src_label: 3,
            src_vid: VertexId::from_int64(1),
            dst_label: 4,
            dst_vid: VertexId::from_int64(2),
            edge_label: 5,
            rank: 0,
        };

        recorder
            .record_mutation(MutationResult {
                entity_keys: vec![
                    MutationEntityKey::Vertex(vertex_id),
                    MutationEntityKey::Edge(edge),
                ],
                ..MutationResult::default()
            })
            .unwrap();

        let write_set = recorder.write_set.lock();
        assert!(write_set.vertices.contains(&vertex_id));
        assert!(write_set.edges.contains(&edge));
    }
}
