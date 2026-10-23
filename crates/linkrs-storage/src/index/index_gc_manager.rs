//! Index Garbage Collection Manager
//!
//! Provides background GC scheduling for **Secondary Index** tombstone cleanup.
//! Integrates with VersionManager to determine safe GC timestamps.
//!
//! ## Property Index GC
//!
//! Secondary indexes use MVCC and require tombstone GC:
//! - `VertexIndexManager`: Supports MVCC, requires GC
//! - `EdgeIndexManager`: Supports MVCC, requires GC
//!
//! This manager handles tombstone cleanup for property indexes only.
//!
//! ## Features
//!
//! - Background GC task scheduling
//! - Incremental GC execution with configurable batch size
//! - Rate limiting to avoid impacting normal operations
//! - Integration with VersionManager for safe timestamp determination
//!
//! ## Usage Example
//!
//! ```rust,ignore
//! use linkrs::storage::index::{IndexGcManager, IndexGcConfig};
//! use linkrs::transaction::VersionManager;
//!
//! let version_manager = Arc::new(VersionManager::new());
//! let index_manager = IndexDataManagerImpl::new();
//!
//! let config = IndexGcConfig::default();
//! let gc_manager = IndexGcManager::new(index_manager, version_manager, config);
//!
//! // Start background GC
//! let handle = gc_manager.start_background_gc();
//!
//! // Later, stop GC
//! gc_manager.stop();
//! handle.join().unwrap();
//! ```

use crate::engine::storage_events::GcEventSink;
use crate::maintenance_scheduler::{
    MaintenanceDiagnostics, MaintenanceTask, MaintenanceTaskConfig,
};
use crate::thread_pool::{BackgroundTaskHandle, StorageThreadPool};
use linkrs_core::types::Timestamp;
use linkrs_transaction::VersionManager;
use parking_lot::{Mutex, RwLock};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::index::traits::IndexGcOps;
use crate::index::types::{GcStats, IndexIdentity};
use crate::index::IndexDataManagerImpl;

/// GC manager configuration
#[derive(Debug, Clone)]
pub struct IndexGcConfig {
    /// Number of entries to process per GC pass
    pub batch_size: usize,
    /// Interval between GC passes in milliseconds
    pub interval_ms: u64,
    /// Minimum interval between GC passes in milliseconds
    pub min_interval_between_gc_ms: u64,
    /// Safety margin for GC timestamp (subtract from safe_ts)
    pub timestamp_margin: Timestamp,
    /// Maximum tombstone count before triggering aggressive GC
    pub tombstone_threshold: usize,
    /// Enable aggressive GC when threshold exceeded
    pub aggressive_gc_enabled: bool,
    /// Tombstone ratio (percentage) above which generational compaction is triggered
    pub compaction_threshold: usize,
    /// Enable generational compaction to remove tombstones
    pub compaction_enabled: bool,
}

impl Default for IndexGcConfig {
    fn default() -> Self {
        Self {
            batch_size: 1000,
            interval_ms: 1000,
            min_interval_between_gc_ms: 100,
            timestamp_margin: 1,
            tombstone_threshold: 10000,
            aggressive_gc_enabled: true,
            compaction_threshold: 30,
            compaction_enabled: true,
        }
    }
}

impl IndexGcConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    pub fn with_interval(mut self, interval_ms: u64) -> Self {
        self.interval_ms = interval_ms;
        self
    }

    pub fn with_tombstone_threshold(mut self, threshold: usize) -> Self {
        self.tombstone_threshold = threshold;
        self
    }

    pub fn with_timestamp_margin(mut self, margin: Timestamp) -> Self {
        self.timestamp_margin = margin;
        self
    }

    pub fn with_compaction_threshold(mut self, threshold: usize) -> Self {
        self.compaction_threshold = threshold;
        self
    }

    pub fn with_compaction_enabled(mut self, enabled: bool) -> Self {
        self.compaction_enabled = enabled;
        self
    }
}

/// Index GC Manager
///
/// Manages background garbage collection for index tombstones.
/// Uses incremental GC to avoid blocking normal operations.
pub struct IndexGcManager {
    index_manager: IndexDataManagerImpl,
    version_manager: Arc<VersionManager>,
    config: IndexGcConfig,
    pool: Arc<StorageThreadPool>,
    last_gc_ts: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
    stats: Arc<AtomicU64>,
    total_removed: Arc<AtomicU64>,
    /// Unified scheduling state shared by the background loop and the
    /// opportunistic write-path trigger: single-flight, completion-time
    /// cooldown, exponential no-progress backoff, panic-safe execution.
    maintenance: Arc<MaintenanceTask>,
    /// Retained periodic-loop handle so [`Self::stop`] can join the loop
    /// even when the caller dropped the handle returned by
    /// [`Self::start_background_gc`]. Shared across clones: the background
    /// task runs on a clone, so the originating handle must observe the
    /// same loop.
    gc_loop: Arc<Mutex<Option<BackgroundTaskHandle>>>,
    gc_event_sink: Arc<RwLock<Option<GcEventSink>>>,
}

impl IndexGcManager {
    /// Create a new GC manager
    pub fn new(
        index_manager: IndexDataManagerImpl,
        version_manager: Arc<VersionManager>,
        config: IndexGcConfig,
        pool: Arc<StorageThreadPool>,
    ) -> Self {
        let maintenance = Arc::new(MaintenanceTask::new(
            pool.clone(),
            MaintenanceTaskConfig::new(
                "index-gc",
                Duration::from_millis(config.min_interval_between_gc_ms),
                Duration::from_secs(30),
            ),
        ));
        Self {
            index_manager,
            version_manager,
            config,
            pool,
            last_gc_ts: Arc::new(AtomicU64::new(0)),
            running: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(AtomicU64::new(0)),
            total_removed: Arc::new(AtomicU64::new(0)),
            maintenance,
            gc_loop: Arc::new(Mutex::new(None)),
            gc_event_sink: Arc::new(RwLock::new(None)),
        }
    }

    /// Attach a sink for reclaimed-entry counts (forwarded as `GcRun`).
    /// The sink is shared across clones so background tasks observe it.
    pub fn set_gc_event_sink(&self, sink: GcEventSink) {
        *self.gc_event_sink.write() = Some(sink);
    }

    fn emit_gc_event(&self, reclaimed_entries: u64) {
        if reclaimed_entries == 0 {
            return;
        }
        if let Some(sink) = self.gc_event_sink.read().clone() {
            sink(reclaimed_entries);
        }
    }

    /// Run a single GC pass
    ///
    /// Returns the number of entries removed. Uses the unified `GcCoordinator`
    /// so the cutoff is fixed at pass start and shared with other table types.
    pub fn run_gc_pass(&self) -> GcStats {
        let gc = crate::engine::gc_coordinator::GcCoordinator::new(self.version_manager.clone())
            .with_margin(self.config.timestamp_margin);
        let diagnostics = gc.diagnostics();
        let safe_ts = diagnostics.safe_gc_timestamp;

        if safe_ts == 0 {
            return GcStats::default();
        }

        let stats = self
            .index_manager
            .gc_tombstones_incremental(safe_ts, self.config.batch_size)
            .unwrap_or_default();

        self.last_gc_ts.store(safe_ts, Ordering::Release);
        self.total_removed
            .fetch_add(stats.total_removed() as u64, Ordering::Release);

        // After tombstone GC, check if generational compaction is needed
        if self.needs_compaction() {
            let compacted = self.run_compaction(safe_ts);
            if compacted > 0 {
                tracing::info!(
                    indexes_compacted = compacted,
                    "Generational compaction completed"
                );
            }
        }

        // Retire generations whose max_ts is past the safe timestamp
        let retired = self.index_manager.retire_generations(safe_ts);
        if retired > 0 {
            tracing::info!(
                generations_retired = retired,
                "Generation retirement completed"
            );
        }

        self.emit_gc_event(stats.total_removed() as u64);
        stats
    }

    /// Run aggressive GC until no more tombstones can be removed
    ///
    /// Returns the total number of entries removed.
    pub fn run_aggressive_gc(&self) -> usize {
        let mut total_removed = 0usize;
        let gc = crate::engine::gc_coordinator::GcCoordinator::new(self.version_manager.clone())
            .with_margin(self.config.timestamp_margin);
        let safe_ts = gc.safe_gc_timestamp();

        if safe_ts == 0 {
            return 0;
        }

        loop {
            let stats = self
                .index_manager
                .gc_tombstones_incremental(safe_ts, self.config.batch_size)
                .unwrap_or_default();

            if stats.is_empty() {
                break;
            }

            total_removed += stats.total_removed();

            if stats.total_removed() < self.config.batch_size {
                break;
            }
        }

        self.total_removed
            .fetch_add(total_removed as u64, Ordering::Release);
        self.emit_gc_event(total_removed as u64);
        total_removed
    }

    /// Get current tombstone count
    pub fn tombstone_count(&self) -> usize {
        self.index_manager.tombstone_count()
    }

    /// Check if aggressive GC is needed
    pub fn needs_aggressive_gc(&self) -> bool {
        self.config.aggressive_gc_enabled
            && self.tombstone_count() > self.config.tombstone_threshold
    }

    /// Check if generational compaction is needed based on tombstone ratio
    pub fn needs_compaction(&self) -> bool {
        if !self.config.compaction_enabled {
            return false;
        }
        let tombstones = self.tombstone_count();
        if tombstones == 0 {
            return false;
        }
        let active = self.index_manager.active_entry_count().max(1);
        let ratio = tombstones * 100 / active;
        ratio > self.config.compaction_threshold
    }

    /// Run generational compaction on all indexes
    ///
    /// Returns the number of indexes compacted.
    pub fn run_compaction(&self, safe_ts: Timestamp) -> usize {
        if safe_ts == 0 {
            return 0;
        }
        let identities: Vec<IndexIdentity> =
            self.index_manager.runtimes.read().keys().copied().collect();
        let mut compacted = 0;
        for identity in identities {
            match self.index_manager.compact_native_index(identity, safe_ts) {
                Ok(true) => compacted += 1,
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!("Compaction failed for index {}: {}", identity.index_id, e);
                }
            }
        }
        compacted
    }

    /// Start the background GC task on the shared thread pool.
    ///
    /// Idempotent: a second call while the loop is scheduled returns the
    /// retained handle instead of spawning a duplicate loop. The returned
    /// handle may be dropped; the loop is retained internally and joined by
    /// [`Self::stop`]. Every pass goes through the unified scheduler:
    /// completion-time cooldown, exponential no-progress backoff,
    /// panic-safe execution, and timeout observation. Throttled passes are
    /// skipped, not counted.
    pub fn start_background_gc(&self) -> BackgroundTaskHandle {
        {
            let guard = self.gc_loop.lock();
            if let Some(handle) = guard.as_ref() {
                if handle.is_running() {
                    return handle.clone();
                }
            }
        }
        let manager = self.clone();
        let running = self.running.clone();
        let interval = Duration::from_millis(self.config.interval_ms);
        let min_interval = Duration::from_millis(self.config.min_interval_between_gc_ms);

        tracing::info!("Index GC background loop started");
        let handle = self
            .pool
            .spawn_periodic(running, interval, min_interval, move || {
                tracing::debug!("Index GC pass started");

                let executed = manager.maintenance.tick(|| {
                    if manager.needs_aggressive_gc() {
                        let removed = manager.run_aggressive_gc();
                        if removed > 0 {
                            tracing::debug!(entries_removed = removed, "Aggressive GC completed");
                        }
                        removed > 0
                    } else {
                        let stats = manager.run_gc_pass();
                        if !stats.is_empty() {
                            tracing::debug!(
                                vertex_removed = stats.vertex_entries_removed,
                                "GC pass completed"
                            );
                        }
                        !stats.is_empty()
                    }
                });
                if executed.is_some() {
                    manager.stats.fetch_add(1, Ordering::Release);
                }
            });
        *self.gc_loop.lock() = Some(handle.clone());
        handle
    }

    /// Opportunistic write-path trigger: schedule one GC pass on the shared
    /// thread pool instead of running it synchronously. Returns `false`
    /// without spawning when a pass is already in flight or the
    /// completion-time cooldown (with backoff) has not elapsed.
    pub fn schedule_opportunistic_gc(&self) -> bool {
        let manager = self.clone();
        self.maintenance
            .try_spawn(move || !manager.run_gc_pass().is_empty())
    }

    /// Unified scheduling state for this GC task: running flag, backoff
    /// step, completion age, run counts, and last-pass progress.
    pub fn maintenance_task(&self) -> &Arc<MaintenanceTask> {
        &self.maintenance
    }

    /// Point-in-time scheduling diagnostics for observability.
    pub fn maintenance_diagnostics(&self) -> MaintenanceDiagnostics {
        self.maintenance.diagnostics()
    }

    /// Total entries removed across all passes (shared with the background
    /// clone so the originating handle stays current).
    pub fn total_removed(&self) -> u64 {
        self.total_removed.load(Ordering::Acquire)
    }

    /// Number of executed GC passes (throttled passes are not counted).
    pub fn pass_count(&self) -> u64 {
        self.stats.load(Ordering::Acquire)
    }

    /// Stop the background GC loop and block until its thread exits.
    ///
    /// The loop handle is retained internally, so this joins even when the
    /// caller dropped the handle returned by [`Self::start_background_gc`].
    /// No `Drop` join: the manager is shared across clones (`Arc`), and a
    /// per-clone `Drop` would stall unrelated owners. Callers that need
    /// deterministic shutdown call `stop()` explicitly.
    pub fn stop(&self) {
        self.running.store(false, Ordering::Release);
        let handle = self.gc_loop.lock().take();
        if let Some(handle) = handle {
            handle.stop();
            handle.join();
        }
    }

    /// Check if the background GC is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

impl Clone for IndexGcManager {
    /// Clones share every counter and the scheduling state: the background
    /// task runs on a clone, so snapshotting counters here would leave the
    /// originating handle permanently stale.
    fn clone(&self) -> Self {
        Self {
            index_manager: self.index_manager.clone(),
            version_manager: self.version_manager.clone(),
            config: self.config.clone(),
            pool: self.pool.clone(),
            last_gc_ts: self.last_gc_ts.clone(),
            running: self.running.clone(),
            stats: self.stats.clone(),
            total_removed: self.total_removed.clone(),
            maintenance: self.maintenance.clone(),
            gc_loop: self.gc_loop.clone(),
            gc_event_sink: self.gc_event_sink.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::index::*;
    use crate::thread_pool::StorageThreadPool;
    use linkrs_transaction::VersionManager;

    #[test]
    fn test_gc_config_default() {
        let config = IndexGcConfig::default();
        assert_eq!(config.batch_size, 1000);
        assert_eq!(config.interval_ms, 1000);
    }

    #[test]
    fn test_gc_config_builder() {
        let config = IndexGcConfig::new()
            .with_batch_size(500)
            .with_interval(500)
            .with_tombstone_threshold(5000);

        assert_eq!(config.batch_size, 500);
        assert_eq!(config.interval_ms, 500);
        assert_eq!(config.tombstone_threshold, 5000);
    }

    #[test]
    fn test_gc_manager_creation() {
        let version_manager = Arc::new(VersionManager::new());
        let index_manager = IndexDataManagerImpl::new();
        let pool = Arc::new(StorageThreadPool::new().unwrap());
        let gc_manager = IndexGcManager::new(
            index_manager,
            version_manager,
            IndexGcConfig::default(),
            pool,
        );

        assert!(!gc_manager.is_running());
        assert_eq!(gc_manager.tombstone_count(), 0);
    }

    #[test]
    fn test_gc_pass_empty() {
        let version_manager = Arc::new(VersionManager::new());
        let index_manager = IndexDataManagerImpl::new();
        let pool = Arc::new(StorageThreadPool::new().unwrap());
        let gc_manager = IndexGcManager::new(
            index_manager,
            version_manager,
            IndexGcConfig::default(),
            pool,
        );

        let stats = gc_manager.run_gc_pass();
        assert!(stats.is_empty());
    }

    #[test]
    fn background_loop_start_is_idempotent_and_stop_joins() {
        use std::time::Duration;
        let version_manager = Arc::new(VersionManager::new());
        let index_manager = IndexDataManagerImpl::new();
        let pool = Arc::new(StorageThreadPool::new().unwrap());
        let config = IndexGcConfig {
            interval_ms: 20,
            min_interval_between_gc_ms: 5,
            ..IndexGcConfig::default()
        };
        let gc_manager = IndexGcManager::new(index_manager, version_manager, config, pool);

        let first = gc_manager.start_background_gc();
        assert!(gc_manager.is_running());
        // Second start while scheduled returns the retained loop, not a
        // duplicate.
        let second = gc_manager.start_background_gc();
        assert!(second.is_running());
        // Handles may be dropped: the loop is retained internally and
        // `stop` still joins it.
        drop(first);
        drop(second);
        std::thread::sleep(Duration::from_millis(50));
        gc_manager.stop();
        assert!(!gc_manager.is_running());
    }
}
