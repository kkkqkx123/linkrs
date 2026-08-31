use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::engine::cache_manager::CacheManager;
use crate::engine::config::PropertyGraphConfig;
use crate::engine::data_store::GraphDataStore;
use crate::engine::paths::StoragePaths;
use crate::engine::persistence_coordinator::PersistenceCoordinator;
use crate::engine::resource_budget::{MemoryAccounting, MemoryBudget};
use crate::engine::spiller::Spiller;
use crate::index::IndexDataManagerImpl;
use crate::vertex::IdKey;
use graphdb_core::metadata::{IndexManager, SchemaManager};
use graphdb_core::stats::StatsManager;
use graphdb_core::types::{LabelId, TableTracker, TableTrackerConfig};
use graphdb_core::UserStorage;
use graphdb_transaction::VersionManager;

use super::evidence::{LayoutVersion, VertexIdDomainEvidence};

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
pub(crate) struct GraphStoragePersistent {
    pub(crate) data_store: Arc<GraphDataStore>,
    pub(crate) cache_manager: Arc<CacheManager>,
    pub(crate) table_tracker: Arc<TableTracker>,
    pub(crate) config: PropertyGraphConfig,
    pub(crate) is_open: Arc<AtomicBool>,
    pub(crate) last_compacted_vertices: LastCompactedVertices,
    pub(crate) index_data_manager: Arc<RwLock<IndexDataManagerImpl>>,
    pub(crate) schema_manager: Arc<SchemaManager>,
    pub(crate) index_metadata_manager: Arc<IndexManager>,
    pub(crate) version_manager: Arc<VersionManager>,
    pub(crate) user_storage: Arc<UserStorage>,
    pub(crate) persistence: Option<Arc<RwLock<PersistenceCoordinator>>>,
    pub(crate) resource_accounting: Arc<MemoryAccounting>,
    pub(crate) spiller: Arc<Spiller>,
    pub(crate) layout: super::GraphStorageLayout,
    pub(crate) stats_manager: Option<Arc<StatsManager>>,
    pub(crate) next_auto_transaction_id: Arc<AtomicU64>,
    /// Serializes auto-commit DML statements (see [`super::autocommit::AutoCommitWriteGate`]).
    pub(crate) auto_commit_write_gate: Arc<super::autocommit::AutoCommitWriteGate>,
    pub(crate) staged_wal: Arc<
        dashmap::DashMap<
            graphdb_core::types::TransactionId,
            Vec<graphdb_transaction::wal::TransactionWalEntry>,
        >,
    >,
    /// Monotonic physical layout version for stale-plan detection.
    pub(crate) layout_version: LayoutVersion,
    /// Self-proven vertex-id domains keyed by label (see
    /// [`VertexIdDomainEvidence`]).
    pub(crate) vertex_id_domains:
        Arc<RwLock<std::collections::HashMap<LabelId, Arc<VertexIdDomainEvidence>>>>,
    /// SERIAL column allocators (one counter per space + table).
    pub(crate) serial_allocator: crate::engine::graph_storage::serial::SerialAllocator,
    pub(crate) migration_history:
        Arc<RwLock<crate::migration_history::MigrationHistoryManager>>,
}

impl GraphStoragePersistent {
    pub(crate) fn dirty_flush_threshold(config: &PropertyGraphConfig) -> usize {
        match usize::try_from(config.resources.dirty_flush_operations) {
            Ok(value) => value,
            Err(_) => usize::MAX,
        }
    }

    pub(crate) fn build_core_components(
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

    pub fn new_with_config(config: PropertyGraphConfig) -> graphdb_core::StorageResult<Self> {
        config.validate()?;
        Ok(Self::from_validated_config(config))
    }

    pub(crate) fn from_validated_config(config: PropertyGraphConfig) -> Self {
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
            layout: super::GraphStorageLayout::new(),
            stats_manager: None,
            // SQLite stores transaction IDs in signed INTEGER columns for the
            // durable outbox. Keep auto-generated IDs in the non-negative
            // i64 domain while leaving room below the high bit for callers.
            next_auto_transaction_id: Arc::new(AtomicU64::new(1 << 62)),
            auto_commit_write_gate: super::autocommit::AutoCommitWriteGate::new(),
            staged_wal: Arc::new(dashmap::DashMap::new()),
            layout_version: LayoutVersion::new(),
            vertex_id_domains: Arc::new(RwLock::new(std::collections::HashMap::new())),
            serial_allocator: crate::engine::graph_storage::serial::SerialAllocator::new(),
            migration_history: Arc::new(RwLock::new(
                crate::migration_history::MigrationHistoryManager::new(),
            )),
        }
    }

    pub(crate) fn new() -> Self {
        Self::from_validated_config(PropertyGraphConfig::default())
    }

    pub(crate) fn new_resource_accounting(config: &PropertyGraphConfig) -> Arc<MemoryAccounting> {
        let resources = &config.resources;
        let budget = MemoryBudget::from_validated(
            resources.max_memory_bytes,
            resources.index_memory_bytes,
            resources.memory_soft_ratio,
            resources.memory_hard_ratio,
        );
        Arc::new(MemoryAccounting::new(budget))
    }

    pub(crate) fn new_spiller(
        config: &PropertyGraphConfig,
        accounting: &Arc<MemoryAccounting>,
        data_store: &Arc<GraphDataStore>,
        cache_manager: &Arc<CacheManager>,
    ) -> Arc<Spiller> {
        let spill_dir = super::GraphStorageLayout::new().spill_dir();
        Arc::new(Spiller::new(
            spill_dir,
            Arc::clone(accounting),
            Arc::clone(data_store),
            Arc::clone(cache_manager),
            config.resources.spill_threshold_ratio,
        ))
    }

    pub(crate) fn new_with_persistence(
        path: PathBuf,
        config: crate::engine::PersistenceConfig,
    ) -> graphdb_core::StorageResult<Self> {
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

        let layout = super::GraphStorageLayout::new_with_path(path);
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
            auto_commit_write_gate: super::autocommit::AutoCommitWriteGate::new(),
            staged_wal: Arc::new(dashmap::DashMap::new()),
            layout_version: LayoutVersion::new(),
            vertex_id_domains: Arc::new(RwLock::new(std::collections::HashMap::new())),
            serial_allocator: crate::engine::graph_storage::serial::SerialAllocator::new(),
            migration_history: Arc::new(RwLock::new(
                crate::migration_history::MigrationHistoryManager::new(),
            )),
        })
    }
}
