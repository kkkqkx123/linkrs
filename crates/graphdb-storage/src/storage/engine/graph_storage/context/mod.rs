use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::core::metadata::{IndexManager, SchemaManager};
use crate::transaction::VersionManager;
use crate::core::types::{
    LabelId, TableTracker, TableTrackerConfig, Timestamp,
    TransactionContextInfo,
};
use crate::core::stats::StatsManager;
use crate::core::UserStorage;
use crate::storage::engine::background_freeze::BackgroundFreezeManager;
use crate::storage::engine::cache_manager::CacheManager;
use crate::storage::engine::config::PropertyGraphConfig;
use crate::storage::engine::data_store::GraphDataStore;
use crate::storage::engine::paths::StoragePaths;
use crate::storage::engine::persistence_coordinator::PersistenceCoordinator;
use crate::storage::index::{IndexDataManagerImpl, IndexGcConfig, IndexGcManager};
use crate::storage::vertex::IdKey;

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
    layout: GraphStorageLayout,
    stats_manager: Option<Arc<StatsManager>>,
}

impl GraphStoragePersistent {
    fn build_core_components() -> CoreComponents {
        let config = PropertyGraphConfig::default();
        let cache_manager = Arc::new(CacheManager::new(config.enable_cache, config.cache_memory));
        let table_tracker = Arc::new(TableTracker::with_config(TableTrackerConfig {
            flush_threshold: config.flush_config.flush_threshold,
            flush_interval: config.flush_config.flush_interval,
        }));

        (
            Arc::new(GraphDataStore::new()),
            cache_manager,
            table_tracker,
            Arc::new(AtomicBool::new(true)),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(RwLock::new(IndexDataManagerImpl::new())),
            Arc::new(SchemaManager::new()),
            Arc::new(IndexManager::new()),
            Arc::new(VersionManager::new()),
            Arc::new(UserStorage::new()),
        )
    }

    fn new_with_config(config: PropertyGraphConfig) -> Self {
        let cache_manager = CacheManager::new(config.enable_cache, config.cache_memory);
        let table_tracker = Arc::new(TableTracker::with_config(TableTrackerConfig {
            flush_threshold: config.flush_config.flush_threshold,
            flush_interval: config.flush_config.flush_interval,
        }));

        Self {
            data_store: Arc::new(GraphDataStore::new()),
            cache_manager: Arc::new(cache_manager),
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
            layout: GraphStorageLayout::new(),
            stats_manager: None,
        }
    }

    fn new() -> Self {
        Self::new_with_config(PropertyGraphConfig::default())
    }

    fn new_with_persistence(
        path: PathBuf,
        config: crate::storage::engine::PersistenceConfig,
    ) -> crate::core::StorageResult<Self> {
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
        ) = Self::build_core_components();

        let persistence = PersistenceCoordinator::new(config).map(|p| Arc::new(RwLock::new(p)))?;

        Ok(Self {
            data_store,
            cache_manager,
            table_tracker,
            config: PropertyGraphConfig::default(),
            is_open,
            last_compacted_vertices,
            index_data_manager,
            schema_manager,
            index_metadata_manager,
            version_manager,
            user_storage,
            persistence: Some(persistence),
            layout: GraphStorageLayout::new_with_path(path),
            stats_manager: None,
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
    current_txn_context: Arc<RwLock<Option<Arc<TransactionContextInfo>>>>,
    index_gc_manager: Option<Arc<IndexGcManager>>,
    background_freeze_manager: Option<Arc<BackgroundFreezeManager>>,
    deferred_wal_ops: DeferredWalOps,
}

impl GraphStorageRuntime {
    fn new() -> Self {
        Self {
            current_txn_context: Arc::new(RwLock::new(None)),
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
            current_txn_context: self.current_txn_context.clone(),
            index_gc_manager: Some(Arc::new(gc_manager)),
            background_freeze_manager: self.background_freeze_manager.clone(),
            deferred_wal_ops: self.deferred_wal_ops.clone(),
        }
    }

    fn with_background_freeze(
        &self,
        manager: Arc<BackgroundFreezeManager>,
    ) -> Self {
        Self {
            current_txn_context: self.current_txn_context.clone(),
            index_gc_manager: self.index_gc_manager.clone(),
            background_freeze_manager: Some(manager),
            deferred_wal_ops: self.deferred_wal_ops.clone(),
        }
    }

    fn get_transaction_context(&self) -> Option<Arc<TransactionContextInfo>> {
        self.current_txn_context.read().clone()
    }

    fn set_transaction_context(&self, context: Option<Arc<TransactionContextInfo>>) {
        *self.current_txn_context.write() = context;
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
}

// ──────────────────────────────────────────────────────────────────────────────
// Module organization: split into logical groups
// ──────────────────────────────────────────────────────────────────────────────

mod mod_init;
mod mod_accessors;
mod mod_schema;
mod mod_vertex_ops;
mod mod_edge_ops;
mod mod_query;
mod mod_persistence;
mod mod_maintenance;
mod mod_cache_index;
mod mod_freeze;
pub(crate) mod helpers;

// Re-export for backward compatibility and internal use
pub use mod_cache_index::ExportedEdgeSnapshotRecord;

impl std::fmt::Debug for GraphStorageContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphStorageContext").finish()
    }
}

impl Default for GraphStorageContext {
    fn default() -> Self {
        Self::new()
    }
}
