use std::sync::atomic::Ordering;
use std::sync::Arc;

use linkrs_core::types::{LabelId, TableId, Timestamp};
use linkrs_core::{StorageError, StorageResult};
use linkrs_metrics::StatsManager;

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
    ) -> Option<Arc<dyn linkrs_transaction::TransactionMutationRecorder>> {
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
        bound.auto_commit_write_set = None;
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
            auto_commit_group_start: None,
            auto_commit_staging_start: None,
            auto_commit_wal_start: 0,
        }));
        Ok(bound)
    }

    pub fn with_auto_commit_context(&self) -> StorageResult<Self> {
        let write_gate_lease = self.persistent.auto_commit_write_gate.acquire();
        let timestamp = self
            .persistent
            .version_manager
            .try_next_write_timestamp()
            .map_err(|error| StorageError::db_error(error.to_string()))?;
        let transaction_id = linkrs_core::types::TransactionId::new(
            self.persistent
                .next_auto_transaction_id
                .fetch_add(1, Ordering::SeqCst),
        );

        let mut bound = self.clone();
        let undo_log = Arc::new(parking_lot::Mutex::new(
            linkrs_transaction::UndoLogManager::new(),
        ));
        let write_set = Arc::new(parking_lot::Mutex::new(
            linkrs_transaction::types::WriteSet::new(),
        ));
        let context = StorageOperationContext {
            transaction_id: Some(transaction_id),
            read_timestamp: timestamp,
            write_timestamp: Some(timestamp),
            read_only: false,
            auto_commit: true,
            mutation_recorder: Some(Arc::new(super::AutoCommitMutationRecorder {
                undo: undo_log.clone(),
                write_set: write_set.clone(),
            })),
            auto_commit_group_start: None,
            auto_commit_staging_start: None,
            auto_commit_wal_start: 0,
        };

        bound.operation_context = Some(Arc::new(context));
        bound.write_timestamp_lease = Some(Arc::new(WriteTimestampLease {
            version_manager: self.persistent.version_manager.clone(),
            timestamp,
            finalized: std::sync::atomic::AtomicBool::new(false),
        }));
        bound.write_gate_lease = Some(write_gate_lease);
        bound.auto_commit_undo = Some(undo_log);
        bound.auto_commit_write_set = Some(write_set);
        Ok(bound)
    }

    pub fn start_index_gc(&self) -> Option<crate::thread_pool::BackgroundTaskHandle> {
        let handle = self.runtime.start_index_gc();
        if handle.is_some() {
            use std::sync::atomic::Ordering;
            self.runtime
                .index_gc_auto_started
                .store(false, Ordering::Release);
        }
        handle
    }

    pub(crate) fn maybe_run_index_gc(&self) {
        // Write-path trigger only schedules: the pass itself runs on the
        // shared pool through the unified scheduler (single-flight,
        // completion-time cooldown, backoff), never inline on the write path.
        if self.runtime.maybe_run_index_gc() {
            log::debug!("Opportunistic index GC scheduled on shared pool");
        } else if let Some(diagnostics) = self.index_gc_maintenance_diagnostics() {
            log::debug!(
                "Opportunistic index GC throttled: {}",
                diagnostics.summary(),
            );
        }
    }

    pub fn stop_index_gc(&self) {
        self.runtime.stop_index_gc();
        use std::sync::atomic::Ordering;
        self.runtime
            .index_gc_user_stopped
            .store(true, Ordering::Release);
        self.runtime
            .index_gc_auto_started
            .store(false, Ordering::Release);
        self.runtime
            .background_pressure_streak
            .store(0, Ordering::Release);
    }

    pub fn is_index_gc_running(&self) -> bool {
        self.runtime.is_index_gc_running()
    }

    pub fn start_vertex_gc(&self) -> Option<crate::thread_pool::BackgroundTaskHandle> {
        let handle = self.runtime.start_vertex_gc();
        if handle.is_some() {
            use std::sync::atomic::Ordering;
            self.runtime
                .vertex_gc_auto_started
                .store(false, Ordering::Release);
        }
        handle
    }

    pub fn stop_vertex_gc(&self) {
        self.runtime.stop_vertex_gc();
        use std::sync::atomic::Ordering;
        self.runtime
            .vertex_gc_user_stopped
            .store(true, Ordering::Release);
        self.runtime
            .vertex_gc_auto_started
            .store(false, Ordering::Release);
        self.runtime
            .background_pressure_streak
            .store(0, Ordering::Release);
    }

    pub fn is_vertex_gc_running(&self) -> bool {
        self.runtime.is_vertex_gc_running()
    }

    /// Explicit gate for self-starting GC loops on sustained pressure.
    /// Off by default; embedded users set it via storage config, server
    /// users via server config. Enabling clears manual-stop suppression.
    pub fn set_background_auto_start(&self, enabled: bool) {
        use std::sync::atomic::Ordering;
        self.runtime
            .background_auto_start
            .store(enabled, Ordering::Release);
        if enabled {
            self.runtime
                .index_gc_user_stopped
                .store(false, Ordering::Release);
            self.runtime
                .vertex_gc_user_stopped
                .store(false, Ordering::Release);
        } else {
            self.runtime
                .background_pressure_streak
                .store(0, Ordering::Release);
        }
    }

    pub(crate) fn apply_background_gate_from_config(&self) {
        let enabled = self
            .persistent
            .config
            .resources
            .background_gc_auto_start_enabled;
        use std::sync::atomic::Ordering;
        self.runtime
            .background_auto_start
            .store(enabled, Ordering::Release);
    }

    pub fn is_background_auto_start_enabled(&self) -> bool {
        use std::sync::atomic::Ordering;
        self.runtime.background_auto_start.load(Ordering::Acquire)
    }

    /// Whether the index GC loop was started automatically by pressure.
    pub fn is_index_gc_auto_started(&self) -> bool {
        use std::sync::atomic::Ordering;
        self.runtime.index_gc_auto_started.load(Ordering::Acquire)
    }

    /// Whether the vertex GC loop was started automatically by pressure.
    pub fn is_vertex_gc_auto_started(&self) -> bool {
        use std::sync::atomic::Ordering;
        self.runtime.vertex_gc_auto_started.load(Ordering::Acquire)
    }

    /// Observe reclamation pressure for one snapshot. Warns when pressure
    /// persists while the corresponding loop stays idle; records the streak
    /// for the optional auto-start gate. Zero behavior change when idle.
    pub(crate) fn observe_background_pressure(
        &self,
        snapshot: &crate::engine::resource_budget::ResourceSnapshot,
    ) {
        use std::sync::atomic::Ordering;
        let index_pressure = snapshot.tombstone_count
            >= self.persistent.config.resources.max_tombstones / 4.max(1);
        let vertex_blocked = self
            .runtime
            .vertex_gc_manager
            .as_ref()
            .map(|gc| gc.blocked_passes())
            .unwrap_or(0);
        let vertex_pressure = vertex_blocked > 0;
        let index_idle = !self.is_index_gc_running();
        let vertex_idle = !self.is_vertex_gc_running();
        let pressured = (index_pressure && index_idle) || (vertex_pressure && vertex_idle);
        if pressured {
            let streak = self
                .runtime
                .background_pressure_streak
                .fetch_add(1, Ordering::AcqRel)
                .saturating_add(1);
            if streak >= 2 {
                log::warn!(
                    "background reclamation pressure sustained (tombstones={} blocked_vertex_passes={} index_running={} vertex_running={} streak={}) while loop idle; enable the loop or raise thresholds",
                    snapshot.tombstone_count,
                    vertex_blocked,
                    !index_idle,
                    !vertex_idle,
                    streak,
                );
            }
            self.maybe_auto_start_background_loops(index_pressure, vertex_pressure);
        } else {
            self.runtime
                .background_pressure_streak
                .store(0, Ordering::Release);
        }
    }

    /// Auto-start idle loops after sustained pressure when the explicit gate
    /// is on. Manual stops suppress future auto-starts until re-enabled.
    fn maybe_auto_start_background_loops(&self, index_pressure: bool, vertex_pressure: bool) {
        use std::sync::atomic::Ordering;
        if !self.is_background_auto_start_enabled() {
            return;
        }
        const AUTO_START_STREAK: u64 = 3;
        if self
            .runtime
            .background_pressure_streak
            .load(Ordering::Acquire)
            < AUTO_START_STREAK
        {
            return;
        }
        if index_pressure
            && !self.is_index_gc_running()
            && !self
                .runtime
                .index_gc_user_stopped
                .load(Ordering::Acquire)
        {
            if self.runtime.start_index_gc().is_some() {
                self.runtime
                    .index_gc_auto_started
                    .store(true, Ordering::Release);
                log::info!("background index GC auto-started on sustained pressure");
            }
        }
        if vertex_pressure
            && !self.is_vertex_gc_running()
            && !self
                .runtime
                .vertex_gc_user_stopped
                .load(Ordering::Acquire)
        {
            if self.runtime.start_vertex_gc().is_some() {
                self.runtime
                    .vertex_gc_auto_started
                    .store(true, Ordering::Release);
                log::info!("background vertex GC auto-started on sustained pressure");
            }
        }
    }

    /// Hint when TTL-tagged schemas exist but the purge sweep never ran.
    /// TTL row counts are unknown without a scan, so this never auto-starts.
    pub(crate) fn ttl_purge_hint(&self) {
        if self.is_ttl_purge_running() {
            return;
        }
        let runs = self
            .ttl_purge_maintenance_diagnostics()
            .map(|d| d.total_runs)
            .unwrap_or(0);
        if runs > 0 {
            return;
        }
        if self.has_ttl_configured() {
            log::warn!(
                "TTL-configured schemas exist but the background purge never ran; expired rows accumulate until PURGE EXPIRED or the opt-in sweep"
            );
        }
    }

    fn has_ttl_configured(&self) -> bool {
        let spaces = match self.persistent.schema_manager.list_spaces() {
            Ok(spaces) => spaces,
            Err(_) => return false,
        };
        for space in &spaces {
            if let Ok(tags) = self.persistent.schema_manager.list_tags(&space.space_name) {
                for tag in &tags {
                    if linkrs_core::types::ttl::has_valid_ttl(
                        tag.ttl_duration,
                        tag.ttl_col.as_deref(),
                    ) {
                        return true;
                    }
                }
            }
            if let Ok(edges) = self
                .persistent
                .schema_manager
                .list_edge_types(&space.space_name)
            {
                for edge in &edges {
                    if linkrs_core::types::ttl::has_valid_ttl(
                        edge.ttl_duration,
                        edge.ttl_col.as_deref(),
                    ) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Start the background TTL purge sweep at `interval`. The scheduler and
    /// its task handle are retained here, so `stop_ttl_purge` can always join
    /// the loop. A zero interval leaves the sweep stopped, and repeated calls
    /// keep the already running sweep.
    pub fn start_ttl_purge(&self, interval: std::time::Duration) {
        let mut guard = self.ttl_purge_scheduler.lock();
        let scheduler = guard.get_or_insert_with(|| {
            crate::engine::graph_storage::ttl_purge::TtlPurgeScheduler::new(
                self.runtime.thread_pool.clone(),
            )
        });
        scheduler.start(interval, self);
    }

    pub fn stop_ttl_purge(&self) {
        if let Some(scheduler) = self.ttl_purge_scheduler.lock().as_mut() {
            scheduler.stop();
        }
    }

    pub fn is_ttl_purge_running(&self) -> bool {
        self.ttl_purge_scheduler
            .lock()
            .as_ref()
            .is_some_and(|scheduler| scheduler.is_running())
    }

    /// Start the idle CSR-maintenance fallback driver at the configured
    /// `background_idle_maintenance_interval`. Auto-started during
    /// construction; a second call keeps the running driver and a zero
    /// interval stays stopped.
    pub fn start_idle_maintenance(&self) {
        let interval = self
            .persistent
            .config
            .resources
            .background_idle_maintenance_interval;
        let thread_pool = self.runtime.thread_pool.clone();
        self.idle_maintenance
            .lock()
            .start(interval, thread_pool, self);
    }

    pub fn stop_idle_maintenance(&self) {
        self.idle_maintenance.lock().stop();
    }

    pub fn is_idle_maintenance_running(&self) -> bool {
        self.idle_maintenance.lock().is_running()
    }

    /// Clone this context for the idle maintenance driver, which must not
    /// own the driver it runs from (that would form a reference cycle
    /// keeping the storage context alive after the last handle drops).
    pub(crate) fn clone_for_idle_maintenance(&self) -> Self {
        let mut dispatch_ctx = self.clone();
        dispatch_ctx.idle_maintenance = Arc::new(parking_lot::Mutex::new(
            super::freeze::IdleMaintenanceDriver::new(),
        ));
        dispatch_ctx
    }

    /// Whether the persistence layer requests one final checkpoint on close.
    pub(crate) fn checkpoint_on_close_enabled(&self) -> bool {
        self.persistent
            .persistence
            .as_ref()
            .is_some_and(|persistence| persistence.read().config.checkpoint_on_close)
    }

    /// Idempotent close: quiesce background loops and, when
    /// `checkpoint_on_close` is configured, persist one final checkpoint.
    ///
    /// The close checkpoint reuses the synchronous checkpoint path; a
    /// failure is logged and never blocks the close. After close, further
    /// operations fail with `storage_not_open`. Explicitly closing is
    /// optional: dropping the last handle stops the drivers without a
    /// final checkpoint.
    pub fn close(&self) {
        if !self.persistent.is_open.swap(false, Ordering::AcqRel) {
            return;
        }
        self.stop_idle_maintenance();
        // Drain in-flight async checkpoints before the final synchronous
        // one so the two never race on the checkpoint directory.
        if let Some(scheduler) = self.checkpoint_scheduler.lock().as_mut() {
            scheduler.stop();
        }
        if self.checkpoint_on_close_enabled() {
            match super::super::persistence::create_checkpoint(self) {
                Ok(Some(stats)) => {
                    log::info!(
                        "Close checkpoint completed: id={} reason=explicit",
                        stats.checkpoint_id,
                    );
                }
                Ok(None) => {
                    log::debug!("Close checkpoint skipped (persistence not enabled)");
                }
                Err(error) => {
                    log::warn!("Close checkpoint failed (ignored): {}", error);
                }
            }
        }
        self.stop_ttl_purge();
        self.runtime.stop_index_gc();
        self.runtime.stop_vertex_gc();
    }

    /// Unified scheduling diagnostics for the CSR background maintenance
    /// task: running flag, backoff step, completion age, run counts, and
    /// last-pass progress.
    pub fn background_maintenance_diagnostics(
        &self,
    ) -> crate::maintenance_scheduler::MaintenanceDiagnostics {
        self.runtime.background_maintenance_task.diagnostics()
    }

    /// Wait until the in-flight CSR background maintenance pass, if any,
    /// finishes. Returns whether idle was reached within `timeout`; a
    /// running pass is never cancelled.
    pub fn wait_for_background_maintenance(&self, timeout: std::time::Duration) -> bool {
        self.runtime
            .background_maintenance_task
            .wait_until_idle(timeout)
    }

    /// Unified scheduling diagnostics for the opportunistic/background index
    /// GC task, or `None` when no index GC manager is configured.
    pub fn index_gc_maintenance_diagnostics(
        &self,
    ) -> Option<crate::maintenance_scheduler::MaintenanceDiagnostics> {
        self.runtime
            .index_gc_manager
            .as_ref()
            .map(|gc| gc.maintenance_diagnostics())
    }

    /// Unified scheduling diagnostics for the vertex GC task, or `None`
    /// when no vertex GC manager is configured.
    pub fn vertex_gc_maintenance_diagnostics(
        &self,
    ) -> Option<crate::maintenance_scheduler::MaintenanceDiagnostics> {
        self.runtime
            .vertex_gc_manager
            .as_ref()
            .map(|gc| gc.maintenance_diagnostics())
    }

    /// Unified scheduling diagnostics for the TTL purge sweep, or `None`
    /// when the sweep was never started.
    pub fn ttl_purge_maintenance_diagnostics(
        &self,
    ) -> Option<crate::maintenance_scheduler::MaintenanceDiagnostics> {
        self.ttl_purge_scheduler
            .lock()
            .as_ref()
            .map(|scheduler| scheduler.maintenance_diagnostics())
    }

    /// One observability snapshot across every independent background task.
    /// Tasks keep their own triggers and intervals; only the scheduling
    /// state (running, backoff, completion age, progress) is unified here.
    pub fn maintenance_overview(&self) -> crate::maintenance_scheduler::MaintenanceOverview {
        let mut tasks = vec![self.background_maintenance_diagnostics()];
        if let Some(diagnostics) = self.index_gc_maintenance_diagnostics() {
            tasks.push(diagnostics);
        }
        if let Some(diagnostics) = self.vertex_gc_maintenance_diagnostics() {
            tasks.push(diagnostics);
        }
        if let Some(diagnostics) = self.ttl_purge_maintenance_diagnostics() {
            tasks.push(diagnostics);
        }
        let (checkpoint_running, checkpoint_pending, checkpoint_active) = self
            .checkpoint_scheduler
            .lock()
            .as_ref()
            .map(|scheduler| {
                (
                    scheduler.is_running(),
                    scheduler.pending(),
                    scheduler.active_count(),
                )
            })
            .unwrap_or((false, false, 0));
        crate::maintenance_scheduler::MaintenanceOverview {
            tasks,
            checkpoint_running,
            checkpoint_pending,
            checkpoint_active,
            ttl_purge_running: self.is_ttl_purge_running(),
            index_gc_loop_running: self.is_index_gc_running(),
            vertex_gc_loop_running: self.is_vertex_gc_running(),
            index_gc_auto_started: self.is_index_gc_auto_started(),
            vertex_gc_auto_started: self.is_vertex_gc_auto_started(),
            idle_maintenance_running: self.is_idle_maintenance_running(),
        }
    }

    /// Clone this context for the background purge sweep, which must not own
    /// the scheduler it runs on (that would form a reference cycle keeping
    /// the storage context alive after the last handle drops).
    pub(crate) fn clone_for_ttl_purge(&self) -> Self {
        let mut sweep_ctx = self.clone();
        sweep_ctx.ttl_purge_scheduler = Arc::new(parking_lot::Mutex::new(None));
        sweep_ctx
    }

    pub fn mark_vertex_modified(&self, label: LabelId) {
        self.persistent
            .table_tracker
            .mark_modified(TableId::vertex(label));
    }

    /// Record-cache bookkeeping for a freshly inserted vertex ID. Writer
    /// paths outside the context module use this instead of reaching into
    /// the persistent state directly.
    pub(crate) fn cache_inserted_vertex_id(
        &self,
        label: LabelId,
        external_id: &str,
        internal_id: u32,
        ts: Timestamp,
    ) {
        self.persistent
            .cache_manager
            .cache_vertex_id(label, external_id, internal_id, ts);
    }

    pub fn mark_edge_modified(&self, label: LabelId) {
        self.persistent
            .table_tracker
            .mark_modified(TableId::edge(label));
    }

    pub fn wal_metrics(&self) -> Option<crate::WalMetrics> {
        let persistence = self.persistent.persistence.as_ref()?;
        let wal = persistence.read().wal_manager()?;
        let metrics = wal.read().metrics();
        Some(metrics)
    }

    /// Force the WAL to a durable state. Write-ahead semantics require a redo
    /// record to be durable before the operation that produced it is
    /// acknowledged; the async flush buffer alone cannot guarantee that.
    /// No-op when persistence is disabled.
    pub(crate) fn sync_wal(&self) -> StorageResult<()> {
        if let Some(persistence) = self.persistent.persistence.as_ref() {
            if let Some(wal) = persistence.read().wal_manager() {
                wal.read().sync()?;
            }
        }
        Ok(())
    }

    pub(crate) fn is_open_flag(&self) -> &std::sync::atomic::AtomicBool {
        &self.persistent.is_open
    }

    pub(crate) fn index_data_manager(
        &self,
    ) -> &parking_lot::RwLock<crate::index::IndexDataManagerImpl> {
        &self.persistent.index_data_manager
    }

    pub(crate) fn schema_manager(&self) -> &Arc<linkrs_core::metadata::SchemaManager> {
        &self.persistent.schema_manager
    }

    pub(crate) fn serial_allocator(
        &self,
    ) -> &crate::engine::graph_storage::serial::SerialAllocator {
        &self.persistent.serial_allocator
    }

    pub(crate) fn index_metadata_manager(&self) -> &Arc<linkrs_core::metadata::IndexManager> {
        &self.persistent.index_metadata_manager
    }

    pub(crate) fn version_manager(&self) -> &Arc<linkrs_transaction::VersionManager> {
        &self.persistent.version_manager
    }

    pub(crate) fn user_storage(&self) -> &Arc<linkrs_core::UserStorage> {
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

    /// Compression used for baseline flushes, so offline tools write probe
    /// and checkpoint content identically to the regular flush path.
    pub(crate) fn flush_compression(&self) -> crate::compression::CompressionType {
        self.persistent.config.flush_config.compression
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
    ) -> linkrs_core::StorageResult<crate::engine::resource_budget::MemoryReservation> {
        self.persistent
            .spiller
            .try_reserve_with_spill(category, bytes)
    }

    pub(crate) fn get_freeze_config_full(&self) -> crate::engine::config::FreezeConfig {
        self.persistent.config.freeze.clone()
    }
}
