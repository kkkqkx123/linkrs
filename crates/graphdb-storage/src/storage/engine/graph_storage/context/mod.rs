use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::core::metadata::{IndexManager, SchemaManager};
use crate::core::stats::StatsManager;
use crate::core::types::{LabelId, TableTracker, TableTrackerConfig, Timestamp};
use crate::core::UserStorage;
use crate::storage::engine::background_freeze::BackgroundFreezeManager;
use crate::storage::engine::cache_manager::CacheManager;
use crate::storage::engine::config::PropertyGraphConfig;
use crate::storage::engine::data_store::GraphDataStore;
use crate::storage::engine::paths::StoragePaths;
use crate::storage::engine::persistence_coordinator::PersistenceCoordinator;
use crate::storage::engine::resource_budget::{MemoryAccounting, MemoryBudget};
use crate::storage::engine::spiller::Spiller;
use crate::storage::index::{IndexDataManagerImpl, IndexGcConfig, IndexGcManager};
use crate::storage::vertex::IdKey;
use crate::storage::StorageOperationContext;
use crate::transaction::VersionManager;

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
            config,
            is_open: Arc::new(AtomicBool::new(true)),
            last_compacted_vertices: Arc::new(Mutex::new(Vec::new())),
            index_data_manager: Arc::new(RwLock::new(IndexDataManagerImpl::new())),
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
    background_freeze_manager: Option<Arc<BackgroundFreezeManager>>,
    deferred_wal_ops: DeferredWalOps,
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

impl GraphStorageRuntime {
    fn new() -> Self {
        Self {
            index_gc_manager: None,
            background_freeze_manager: None,
            deferred_wal_ops: DeferredWalOps::new(),
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
            background_freeze_manager: self.background_freeze_manager.clone(),
            deferred_wal_ops: self.deferred_wal_ops.clone(),
        }
    }

    fn with_background_freeze(&self, manager: Arc<BackgroundFreezeManager>) -> Self {
        Self {
            index_gc_manager: self.index_gc_manager.clone(),
            background_freeze_manager: Some(manager),
            deferred_wal_ops: self.deferred_wal_ops.clone(),
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
}

#[derive(Clone)]
pub struct GraphStorageContext {
    persistent: GraphStoragePersistent,
    runtime: GraphStorageRuntime,
    operation_context: Option<Arc<StorageOperationContext>>,
    write_timestamp_lease: Option<Arc<WriteTimestampLease>>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Module organization: split into logical groups
// ──────────────────────────────────────────────────────────────────────────────

pub(crate) mod helpers;
mod mod_accessors;
mod mod_cache_index;
mod mod_edge_ops;
mod mod_freeze;
mod mod_init;
mod mod_maintenance;
mod mod_persistence;
mod mod_query;
mod mod_schema;
mod mod_vertex_ops;

// Re-export for backward compatibility and internal use
pub use mod_cache_index::ExportedEdgeSnapshotRecord;

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
        })
    }
}

impl Default for GraphStorageContext {
    fn default() -> Self {
        Self::new()
    }
}
