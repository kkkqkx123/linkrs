//! Vertex Table Garbage Collection Manager
//!
//! Provides background GC scheduling for vertex table tombstone cleanup.
//! Periodically scans all vertex tables and reclaims space from
//! deleted vertices that are older than the safe GC timestamp.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::cache::SharedRecordCache;
use crate::engine::data_store::GraphDataStore;
use crate::engine::storage_events::GcEventSink;
use crate::maintenance_scheduler::{
    MaintenanceDiagnostics, MaintenanceTask, MaintenanceTaskConfig,
};
use crate::thread_pool::{BackgroundTaskHandle, StorageThreadPool};
use linkrs_core::types::Timestamp;
use linkrs_transaction::VersionManager;
use parking_lot::{Mutex, RwLock};

/// GC manager configuration
#[derive(Debug, Clone)]
pub struct VertexGcConfig {
    /// Interval between GC passes in milliseconds
    pub interval_ms: u64,
    /// Minimum interval between GC passes in milliseconds
    pub min_interval_between_gc_ms: u64,
    /// Safety margin for GC timestamp (subtract from safe_ts)
    pub timestamp_margin: Timestamp,
}

impl Default for VertexGcConfig {
    fn default() -> Self {
        Self {
            interval_ms: 5000,
            min_interval_between_gc_ms: 500,
            timestamp_margin: 1,
        }
    }
}

impl VertexGcConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_interval(mut self, interval_ms: u64) -> Self {
        self.interval_ms = interval_ms;
        self
    }

    pub fn with_timestamp_margin(mut self, margin: Timestamp) -> Self {
        self.timestamp_margin = margin;
        self
    }
}

/// Vertex Table GC Manager
///
/// Manages background garbage collection for vertex tables.
/// Acquires the vertex table write lock once per GC pass and
/// calls `gc()` on each registered table.
/// Age in seconds beyond which an active snapshot is reported as stuck.
///
/// A pinned snapshot freezes the GC watermark, so version chains grow
/// without bound while it lives. The threshold only drives warnings and
/// blocked-pass accounting, never reclamation: reclaiming below the shared
/// cutoff would expose uncommitted rows to live readers.
pub const STUCK_SNAPSHOT_AGE_SECS: u64 = 30;

/// Whether an oldest-active-snapshot age counts as stuck for GC purposes.
/// Pure predicate so the stuck-watermark policy is unit-testable without
/// a running version manager.
pub fn is_snapshot_stuck(oldest_age_secs: u64) -> bool {
    oldest_age_secs > STUCK_SNAPSHOT_AGE_SECS
}

pub struct VertexGcManager {
    data_store: Arc<GraphDataStore>,
    version_manager: Arc<VersionManager>,
    config: VertexGcConfig,
    pool: Arc<StorageThreadPool>,
    running: Arc<AtomicBool>,
    stats: Arc<AtomicU64>,
    total_removed: Arc<AtomicU64>,
    /// Unified scheduling state for background passes: single-flight,
    /// completion-time cooldown, exponential no-progress backoff,
    /// panic-safe execution.
    maintenance: Arc<MaintenanceTask>,
    /// Retained periodic-loop handle so [`Self::stop`] can join the loop
    /// even when the caller dropped the handle returned by
    /// [`Self::start_background_gc`]. Shared across clones.
    gc_loop: Arc<Mutex<Option<BackgroundTaskHandle>>>,
    /// Passes that reclaimed nothing because the watermark was pinned
    /// (`safe_ts == 0`) or a stuck snapshot held the cutoff. A rising
    /// count beside growing version chains points at the snapshot holder,
    /// not at GC throughput.
    blocked_passes: Arc<AtomicU64>,
    /// Record-cache label invalidations issued by GC passes that reclaimed
    /// vertex keys. Observed to confirm remap/cache-fence coverage after
    /// churn; a pass that reclaims keys without invalidating is a
    /// correctness bug, not a metric gap.
    cache_invalidations: Arc<AtomicU64>,
    /// Passes that released cold chunks to contain resident growth while
    /// version chains were pinned by live snapshots. Chains themselves are
    /// never dropped below the watermark; only evictable chunks (no overlay
    /// writes, no live chains) are externalized, so pinned reads stay
    /// correct while memory converges.
    pressure_mitigations: Arc<AtomicU64>,
    gc_event_sink: Arc<RwLock<Option<GcEventSink>>>,
    /// Record cache to invalidate when a GC pass remaps internal IDs.
    /// Compaction re-densifies the ID space, so cached ID mappings and
    /// vertex records keyed by old internal IDs must be dropped; otherwise
    /// forward-compatible cache hits (`cached_at_ts <= query_ts`) would
    /// serve relocated rows to newer readers.
    record_cache: Arc<RwLock<Option<SharedRecordCache>>>,
}

impl VertexGcManager {
    pub fn new(
        data_store: Arc<GraphDataStore>,
        version_manager: Arc<VersionManager>,
        config: VertexGcConfig,
        pool: Arc<StorageThreadPool>,
    ) -> Self {
        let maintenance = Arc::new(MaintenanceTask::new(
            pool.clone(),
            MaintenanceTaskConfig::new(
                "vertex-gc",
                Duration::from_millis(config.min_interval_between_gc_ms),
                Duration::from_secs(30),
            ),
        ));
        Self {
            data_store,
            version_manager,
            config,
            pool,
            running: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(AtomicU64::new(0)),
            total_removed: Arc::new(AtomicU64::new(0)),
            maintenance,
            gc_loop: Arc::new(Mutex::new(None)),
            blocked_passes: Arc::new(AtomicU64::new(0)),
            cache_invalidations: Arc::new(AtomicU64::new(0)),
            pressure_mitigations: Arc::new(AtomicU64::new(0)),
            gc_event_sink: Arc::new(RwLock::new(None)),
            record_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Attach the record cache so GC passes that remap internal IDs can
    /// invalidate the affected labels. No-op when caching is disabled.
    pub fn set_record_cache(&self, cache: SharedRecordCache) {
        *self.record_cache.write() = Some(cache);
    }

    /// Attach a sink for reclaimed-entry counts (forwarded as `GcRun`).
    /// The sink is shared across clones so background tasks observe it.
    pub fn set_gc_event_sink(&self, sink: GcEventSink) {
        *self.gc_event_sink.write() = Some(sink);
    }

    /// Walk all vertex tables evicting cold chunks up to `budget` bytes.
    /// Tolerant by design: per-table failures only warn. Background batch
    /// loads behind this pass are capped by
    /// [`crate::vertex::column::MAX_BACKGROUND_LOAD_CHUNKS`] chunks per
    /// segment and over-quota eviction proceeds in
    /// [`crate::vertex::column::EVICTION_SEGMENT_BYTES`] segments.
    fn evict_cold_chunks(data_store: &GraphDataStore, budget: u64) {
        use crate::vertex::column::EVICTION_SEGMENT_BYTES;
        let mut remaining = budget;
        let mut evicted = 0usize;
        let mut freed = 0u64;
        let mut segments = 0usize;
        data_store.with_vertex_tables(|tables| {
            for table in tables.values() {
                if remaining == 0 {
                    break;
                }
                let (n, bytes, segs) =
                    table.evict_cold_chunks_with_quota(remaining, EVICTION_SEGMENT_BYTES);
                evicted += n;
                freed += bytes;
                segments += segs;
                remaining = remaining.saturating_sub(bytes);
                if segs == 0 {
                    continue;
                }
            }
        });
        if evicted > 0 {
            let (resident_chunks, evicted_chunks, evicted_bytes, resident_bytes) = data_store
                .with_vertex_tables(|tables| {
                    let mut stats = (0usize, 0usize, 0usize, 0usize);
                    for table in tables.values() {
                        let s = table.eviction_stats();
                        stats.0 += s.0;
                        stats.1 += s.1;
                        stats.2 += s.2;
                        stats.3 += s.3;
                    }
                    stats
                });
            log::info!(
                "chunk eviction under memory pressure: {} chunks, {} bytes released \
                 (resident_chunks={} evicted_chunks={} evicted_bytes={} resident_bytes={} segments={})",
                evicted,
                freed,
                resident_chunks,
                evicted_chunks,
                evicted_bytes,
                resident_bytes,
                segments,
            );
        }
    }

    fn emit_gc_event(&self, reclaimed_entries: u64) {
        if reclaimed_entries == 0 {
            return;
        }
        if let Some(sink) = self.gc_event_sink.read().clone() {
            sink(reclaimed_entries);
        }
    }

    /// Run a single GC pass across all vertex tables.
    ///
    /// Returns the total number of vertex entries removed. Uses the unified
    /// watermark view so all table types in the same pass share the same
    /// cutoff.
    pub fn run_gc_pass(&self) -> usize {
        let coordinator =
            crate::engine::gc_coordinator::GcCoordinator::new(self.version_manager.clone())
                .with_margin(self.config.timestamp_margin);
        let diagnostics = coordinator.diagnostics();
        let watermarks = diagnostics.watermarks;
        let safe_ts = diagnostics.safe_gc_timestamp;

        // The version-manager safe timestamp is the single cutoff
        // authority: transaction-side long reads already pin it through
        // the transaction layer's own lease manager, so storage applies
        // no second floor here.
        let cutoff = safe_ts;

        if cutoff == 0 {
            self.blocked_passes.fetch_add(1, Ordering::Release);
            return 0;
        }
        if watermarks.has_active_snapshot() {
            if let Some(age) = watermarks.oldest_age(&self.version_manager) {
                if is_snapshot_stuck(age.as_secs()) {
                    self.blocked_passes.fetch_add(1, Ordering::Release);
                    log::warn!(
                        "GC blocked by long-lived snapshot age={:?} safe_gc={} oldest_active={} blocked_passes={}",
                        age,
                        safe_ts,
                        watermarks.oldest_active_snapshot,
                        self.blocked_passes.load(Ordering::Acquire),
                    );
                }
            }
        }

        let mut total_removed = 0usize;
        let mut remapped_labels = Vec::new();
        // Snapshots are global to VersionManager; per-table pin counts are
        // always zero by design. Log the pass-wide active count once.
        let pass_active = diagnostics.active_snapshot_count;
        if let Err(e) = self.data_store.with_vertex_tables_mut(|tables| {
            for table in tables.values() {
                match table.gc_detailed(cutoff) {
                    Ok((reclaimed_vertices, version_entries)) => {
                        total_removed += reclaimed_vertices + version_entries;
                        if reclaimed_vertices > 0 {
                            // Stable row ids keep live rows in place; cached
                            // entries for reclaimed keys must still drop so
                            // later reads miss instead of serving deletes.
                            remapped_labels.push(table.label());
                        }
                        if reclaimed_vertices + version_entries > 0 && pass_active > 0 {
                            log::debug!(
                                "GC removed {} entries from vertex table with {} active snapshots",
                                reclaimed_vertices + version_entries,
                                pass_active,
                            );
                        }
                    }
                    Err(err) => {
                        log::warn!("Vertex table GC failed: {}", err);
                    }
                }
            }
            Ok(())
        }) {
            log::warn!("Vertex table GC encountered error: {}", e);
        }

        if !remapped_labels.is_empty() {
            if let Some(cache) = self.record_cache.read().clone() {
                for label in remapped_labels {
                    cache.invalidate_vertices_by_label(label);
                    cache.invalidate_id_indexes_by_label(label);
                    self.cache_invalidations.fetch_add(1, Ordering::Release);
                }
                log::debug!(
                    "GC invalidated record cache: total_invalidations={}",
                    self.cache_invalidations.load(Ordering::Acquire)
                );
            }
        }

        // Active mitigation for pinned-snapshot version pressure: chains
        // cannot fold below live snapshots, so release evictable cold
        // chunks to contain resident growth while the watermark is pinned.
        // Only chunks without overlay writes or live chains are released;
        // the pinned chains themselves are retained, so snapshot reads stay
        // correct. Bounded to one eviction segment per pass.
        let pressured_max_chain = self.data_store.with_vertex_tables(|tables| {
            tables
                .values()
                .map(|table| table.version_chain_pressure().1)
                .max()
                .unwrap_or(0)
        });
        if pressured_max_chain
            > crate::vertex::vertex_table::core::VertexTable::VERSION_CHAIN_PRESSURE_WARN_LEN
        {
            Self::evict_cold_chunks(
                &self.data_store,
                crate::vertex::column::EVICTION_SEGMENT_BYTES,
            );
            let mitigations = self
                .pressure_mitigations
                .fetch_add(1, Ordering::Release)
                .saturating_add(1);
            log::warn!(
                "GC mitigating version-chain pressure: max_chain_len={} mitigations={}",
                pressured_max_chain,
                mitigations,
            );
        }

        // Process-wide watermark no longer drives storage eviction: it is a
        // query-side admission signal for new acceleration structures. Storage
        // relieves pressure through the instance-budget relief chain in
        // `resource_admission`, so this pass only mitigates pinned
        // version-chain growth above.

        self.total_removed
            .fetch_add(total_removed as u64, Ordering::Release);
        self.emit_gc_event(total_removed as u64);
        total_removed
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

        tracing::info!("Vertex GC background loop started");
        let handle = self
            .pool
            .spawn_periodic(running, interval, min_interval, move || {
                tracing::debug!("Vertex GC pass started");

                let executed = manager.maintenance.tick(|| manager.run_gc_pass() > 0);
                if let Some(removed_progress) = executed {
                    if removed_progress {
                        tracing::debug!("Vertex GC pass completed");
                    }
                    manager.stats.fetch_add(1, Ordering::Release);
                }
            });
        *self.gc_loop.lock() = Some(handle.clone());
        handle
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

    /// Point-in-time scheduling diagnostics for observability.
    pub fn maintenance_diagnostics(&self) -> MaintenanceDiagnostics {
        self.maintenance.diagnostics()
    }

    /// Total entries removed since creation
    pub fn total_removed(&self) -> u64 {
        self.total_removed.load(Ordering::Acquire)
    }

    /// Passes that reclaimed nothing because the watermark was pinned.
    pub fn blocked_passes(&self) -> u64 {
        self.blocked_passes.load(Ordering::Acquire)
    }

    /// Record-cache invalidations issued by GC passes.
    pub fn cache_invalidations(&self) -> u64 {
        self.cache_invalidations.load(Ordering::Acquire)
    }

    /// Number of GC passes completed
    pub fn pass_count(&self) -> u64 {
        self.stats.load(Ordering::Acquire)
    }

    /// Passes that released cold chunks to contain resident growth while
    /// version chains were pinned by live snapshots.
    pub fn pressure_mitigations(&self) -> u64 {
        self.pressure_mitigations.load(Ordering::Acquire)
    }
}

impl Clone for VertexGcManager {
    /// Clones share every counter and the scheduling state: the background
    /// task runs on a clone, so snapshotting counters here would leave the
    /// originating handle permanently stale.
    fn clone(&self) -> Self {
        Self {
            data_store: self.data_store.clone(),
            version_manager: self.version_manager.clone(),
            config: self.config.clone(),
            pool: self.pool.clone(),
            running: self.running.clone(),
            stats: self.stats.clone(),
            total_removed: self.total_removed.clone(),
            maintenance: self.maintenance.clone(),
            gc_loop: self.gc_loop.clone(),
            blocked_passes: self.blocked_passes.clone(),
            cache_invalidations: self.cache_invalidations.clone(),
            pressure_mitigations: self.pressure_mitigations.clone(),
            gc_event_sink: self.gc_event_sink.clone(),
            record_cache: self.record_cache.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::data_store::GraphDataStore;

    #[test]
    fn test_gc_config_default() {
        let config = VertexGcConfig::default();
        assert_eq!(config.interval_ms, 5000);
    }

    #[test]
    fn test_gc_config_builder() {
        let config = VertexGcConfig::new()
            .with_interval(2000)
            .with_timestamp_margin(2);
        assert_eq!(config.interval_ms, 2000);
        assert_eq!(config.timestamp_margin, 2);
    }

    #[test]
    fn test_gc_manager_creation() {
        let data_store = Arc::new(GraphDataStore::new());
        let version_manager = Arc::new(VersionManager::new());
        let pool = Arc::new(StorageThreadPool::new().unwrap());
        let gc = VertexGcManager::new(data_store, version_manager, VertexGcConfig::default(), pool);
        assert!(!gc.is_running());
        assert_eq!(gc.total_removed(), 0);
    }

    #[test]
    fn test_stuck_snapshot_threshold() {
        assert!(!is_snapshot_stuck(0));
        assert!(!is_snapshot_stuck(STUCK_SNAPSHOT_AGE_SECS));
        assert!(is_snapshot_stuck(STUCK_SNAPSHOT_AGE_SECS + 1));
    }

    #[test]
    fn test_pressure_mitigation_starts_at_zero_and_clones() {
        use crate::vertex::vertex_table::core::VertexTable;
        assert_eq!(VertexTable::VERSION_CHAIN_PRESSURE_WARN_LEN, 1024);
        let gc = test_manager();
        assert_eq!(gc.pressure_mitigations(), 0);
        assert_eq!(gc.clone().pressure_mitigations(), 0);
    }

    fn test_manager() -> VertexGcManager {
        VertexGcManager::new(
            Arc::new(GraphDataStore::new()),
            Arc::new(VersionManager::new()),
            VertexGcConfig::default(),
            Arc::new(StorageThreadPool::new().unwrap()),
        )
    }

    #[test]
    fn test_clone_shares_counters_with_background_handle() {
        // The background task runs on a clone; counter writes on either
        // handle must be visible from the other, otherwise the originating
        // handle reports permanently stale observability.
        let gc = test_manager();
        let worker = gc.clone();
        worker
            .stats
            .fetch_add(1, std::sync::atomic::Ordering::Release);
        assert_eq!(gc.pass_count(), 1);
        worker
            .total_removed
            .fetch_add(7, std::sync::atomic::Ordering::Release);
        assert_eq!(gc.total_removed(), 7);
    }

    #[test]
    fn background_loop_start_is_idempotent_and_stop_joins() {
        let config = VertexGcConfig {
            interval_ms: 20,
            min_interval_between_gc_ms: 5,
            ..VertexGcConfig::default()
        };
        let gc = VertexGcManager::new(
            Arc::new(GraphDataStore::new()),
            Arc::new(VersionManager::new()),
            config,
            Arc::new(StorageThreadPool::new().unwrap()),
        );
        let first = gc.start_background_gc();
        assert!(gc.is_running());
        let second = gc.start_background_gc();
        assert!(second.is_running());
        drop(first);
        drop(second);
        std::thread::sleep(Duration::from_millis(50));
        gc.stop();
        assert!(!gc.is_running());
    }
}
