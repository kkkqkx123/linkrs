use std::path::PathBuf;
use std::sync::Arc;

use crate::core::StorageResult;
use crate::storage::engine::background_freeze::BackgroundFreezeManager;
use crate::storage::index::IndexGcConfig;
use crate::storage::engine::PersistenceConfig;
use crate::core::stats::StatsManager;

use super::{GraphStorageContext, GraphStoragePersistent, GraphStorageRuntime};

impl GraphStorageContext {
    pub fn new() -> Self {
        Self {
            persistent: GraphStoragePersistent::new(),
            runtime: GraphStorageRuntime::new(),
        }
    }

    pub fn new_with_path(path: PathBuf) -> StorageResult<Self> {
        let config = crate::storage::engine::PersistenceConfig::for_work_dir(&path);
        Self::new_with_persistence(path, config)
    }

    pub fn new_with_persistence(
        path: PathBuf,
        config: PersistenceConfig,
    ) -> StorageResult<Self> {
        GraphStoragePersistent::new_with_persistence(path, config).map(|persistent| Self {
            persistent,
            runtime: GraphStorageRuntime::new(),
        })
    }

    pub fn with_index_gc(mut self, config: IndexGcConfig) -> Self {
        let runtime = self.runtime.with_index_gc(
            &self.persistent.index_data_manager,
            &self.persistent.version_manager,
            config,
        );
        self.runtime = runtime;
        self
    }

    pub fn with_background_freeze(
        &self,
        manager: Arc<BackgroundFreezeManager>,
    ) -> Self {
        let runtime = self.runtime.with_background_freeze(manager);
        Self {
            persistent: self.persistent.clone(),
            runtime,
        }
    }

    /// Set the StatsManager for recording MVCC metrics to EdgeTable instances.
    ///
    /// This should be called once after creating the GraphStorageContext,
    /// typically at startup time. The stats manager will be injected into all
    /// EdgeTable instances for automatic metrics recording.
    pub fn set_stats_manager(&mut self, stats: Arc<StatsManager>) {
        self.persistent.stats_manager = Some(stats);
    }
}
