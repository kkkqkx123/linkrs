use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::engine::background_freeze::BackgroundFreezeManager;
use crate::engine::PersistenceConfig;
use crate::index::IndexGcConfig;
use crate::vertex::VertexGcConfig;
use graphdb_core::stats::{CheckpointTriggerReason, StatsManager};
use graphdb_core::StorageResult;

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
            checkpoint_scheduler: Arc::new(Mutex::new(None)),
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
            let ctx = Self {
                persistent,
                runtime: GraphStorageRuntime::new(),
                operation_context: None,
                write_timestamp_lease: None,
                write_gate_lease: None,
                auto_commit_undo: None,
                auto_commit_window: None,
                cold_snapshots: Arc::new(RwLock::new(HashMap::new())),
                checkpoint_scheduler: Arc::new(Mutex::new(None)),
            }
            .with_default_index_gc();
            ctx.ensure_checkpoint_scheduler();
            ctx
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
            checkpoint_scheduler: self.checkpoint_scheduler.clone(),
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
            .set_stats_manager(stats.clone());
        // Propagate to checkpoint scheduler if already initialized
        if let Some(scheduler) = self.checkpoint_scheduler.lock().as_ref() {
            scheduler.set_stats(Some(stats.clone()));
        }
        self.ensure_checkpoint_scheduler();
    }

    pub(crate) fn ensure_checkpoint_scheduler(&self) {
        let persistence_opt = self.persistent.persistence.clone();
        let Some(persistence) = persistence_opt else {
            return;
        };
        let (enabled, poll_interval) = {
            let cfg = &persistence.read().config;
            (
                cfg.async_checkpoint_enabled,
                cfg.async_checkpoint_poll_interval,
            )
        };
        if !enabled {
            return;
        }
        let mut guard = self.checkpoint_scheduler.lock();
        if guard.is_some() {
            return;
        }
        let ctx_clone = self.clone();
        let executor: std::sync::Arc<
            dyn Fn(
                    crate::engine::persistence_coordinator::PersistenceStateGuard,
                    CheckpointTriggerReason,
                ) -> StorageResult<crate::CheckpointStats>
                + Send
                + Sync,
        > = std::sync::Arc::new(move |guard, reason| {
            crate::engine::graph_storage::persistence::create_checkpoint_with_guard(
                &ctx_clone, guard, reason,
            )
            .and_then(|opt| {
                opt.ok_or_else(|| {
                    graphdb_core::StorageError::db_error(
                        "checkpoint not available (no persistence)".to_string(),
                    )
                })
            })
        });
        let mut scheduler = crate::engine::persistence_coordinator::CheckpointScheduler::new(
            persistence,
            self.runtime.thread_pool.clone(),
            self.persistent.stats_manager.clone(),
            poll_interval,
            enabled,
            executor,
        );
        scheduler.start();
        *guard = Some(scheduler);
    }

    pub(crate) fn request_async_checkpoint(&self, reason: CheckpointTriggerReason) {
        self.ensure_checkpoint_scheduler();
        if let Some(scheduler) = self.checkpoint_scheduler.lock().as_ref() {
            scheduler.request_checkpoint(reason);
        }
    }

    pub fn is_checkpoint_scheduler_running(&self) -> bool {
        self.checkpoint_scheduler
            .lock()
            .as_ref()
            .map(|s| s.is_running())
            .unwrap_or(false)
    }

    pub fn checkpoint_diagnostics(&self) -> Option<crate::engine::persistence_coordinator::CheckpointDiagnostics> {
        let persistence = self.persistent.persistence.as_ref()?.read();
        let pending = self
            .checkpoint_scheduler
            .lock()
            .as_ref()
            .map(|s| s.pending())
            .unwrap_or(false);
        Some(persistence.checkpoint_diagnostics(pending))
    }
}
