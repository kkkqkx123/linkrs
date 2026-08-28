use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;

use graphdb_core::stats::StatsManager;
use graphdb_core::StorageResult;
use crate::engine::background_freeze::BackgroundFreezeManager;
use crate::engine::PersistenceConfig;
use crate::index::IndexGcConfig;
use crate::vertex::VertexGcConfig;

use super::{GraphStorageContext, GraphStoragePersistent, GraphStorageRuntime};

impl GraphStorageContext {
    pub fn new() -> Self {
        Self {
            persistent: GraphStoragePersistent::new(),
            runtime: GraphStorageRuntime::new(),
            operation_context: None,
            write_timestamp_lease: None,
            write_gate_lease: None,
            auto_commit_undo: None,
            auto_commit_window: None,
            cold_snapshots: Arc::new(RwLock::new(HashMap::new())),
        }
        .with_default_index_gc()
    }

    pub fn new_with_path(path: PathBuf) -> StorageResult<Self> {
        let config = crate::engine::PersistenceConfig::for_work_dir(&path);
        Self::new_with_persistence(path, config)
    }

    pub fn new_with_persistence(path: PathBuf, config: PersistenceConfig) -> StorageResult<Self> {
        GraphStoragePersistent::new_with_persistence(path, config).map(|persistent| {
            if let Err(e) = persistent.spiller.cleanup_stale_files() {
                log::warn!("Failed to clean up stale spill files: {}", e);
            }
            Self {
                persistent,
                runtime: GraphStorageRuntime::new(),
                operation_context: None,
                write_timestamp_lease: None,
                write_gate_lease: None,
                auto_commit_undo: None,
                auto_commit_window: None,
                cold_snapshots: Arc::new(RwLock::new(HashMap::new())),
            }
            .with_default_index_gc()
        })
    }

    /// assemble the index GC manager by default so generation retirement
    /// and reclamation stay bounded. Callers that manage GC themselves can
    /// replace this via [`with_index_gc`](Self::with_index_gc).
    fn with_default_index_gc(self) -> Self {
        self.with_index_gc(IndexGcConfig::default())
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

    pub fn with_vertex_gc(mut self, config: VertexGcConfig) -> Self {
        let runtime = self.runtime.with_vertex_gc(
            &self.persistent.data_store,
            &self.persistent.version_manager,
            config,
        );
        self.runtime = runtime;
        self
    }

    pub fn with_background_freeze(&self, manager: Arc<BackgroundFreezeManager>) -> Self {
        let runtime = self.runtime.with_background_freeze(manager);
        Self {
            persistent: self.persistent.clone(),
            runtime,
            operation_context: self.operation_context.clone(),
            write_timestamp_lease: self.write_timestamp_lease.clone(),
            write_gate_lease: self.write_gate_lease.clone(),
            auto_commit_undo: self.auto_commit_undo.clone(),
            auto_commit_window: self.auto_commit_window.clone(),
            cold_snapshots: self.cold_snapshots.clone(),
        }
    }

    /// Set the StatsManager for recording MVCC metrics to EdgeTable instances.
    ///
    /// This should be called once after creating the GraphStorageContext,
    /// typically at startup time. The stats manager will be injected into all
    /// EdgeTable instances for automatic metrics recording.
    pub fn set_stats_manager(&mut self, stats: Arc<StatsManager>) {
        self.persistent.stats_manager = Some(stats.clone());
        self.persistent
            .index_data_manager
            .write()
            .set_stats_manager(stats);
    }
}
