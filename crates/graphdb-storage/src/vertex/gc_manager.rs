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
use crate::thread_pool::{BackgroundTaskHandle, StorageThreadPool};
use graphdb_core::types::Timestamp;
use graphdb_transaction::VersionManager;
use parking_lot::RwLock;

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
pub struct VertexGcManager {
    data_store: Arc<GraphDataStore>,
    version_manager: Arc<VersionManager>,
    config: VertexGcConfig,
    pool: Arc<StorageThreadPool>,
    running: Arc<AtomicBool>,
    stats: AtomicU64,
    total_removed: AtomicU64,
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
        Self {
            data_store,
            version_manager,
            config,
            pool,
            running: Arc::new(AtomicBool::new(false)),
            stats: AtomicU64::new(0),
            total_removed: AtomicU64::new(0),
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

        if safe_ts == 0 {
            return 0;
        }
        if watermarks.has_active_snapshot() {
            if let Some(age) = watermarks.oldest_age(&self.version_manager) {
                if age.as_secs() > 30 {
                    log::warn!(
                        "GC blocked by long-lived snapshot age={:?} safe_gc={} oldest_active={}",
                        age,
                        safe_ts,
                        watermarks.oldest_active_snapshot
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
                match table.gc_detailed(safe_ts) {
                    Ok((reclaimed_vertices, version_entries)) => {
                        total_removed += reclaimed_vertices + version_entries;
                        if reclaimed_vertices > 0 {
                            // Internal IDs were re-densified: cached ID
                            // mappings and vertex records for this label
                            // are keyed by stale IDs until invalidated.
                            // Version-only passes leave IDs untouched.
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
                }
            }
        }

        // Watermark-triggered chunk eviction on the same background pass:
        // under High/Critical process pressure, release cold encoded column
        // chunks oldest-first so memory converges with the working set.
        // Runs without shard locks held at entry; each shard evicts under
        // its own write lock with a recheck inside.
        match crate::memory_watermark::pressure() {
            crate::memory_watermark::Pressure::Low => {}
            crate::memory_watermark::Pressure::High => {
                Self::evict_cold_chunks(&self.data_store, 64 * 1024 * 1024);
            }
            crate::memory_watermark::Pressure::Critical => {
                Self::evict_cold_chunks(&self.data_store, u64::MAX);
            }
        }

        self.total_removed
            .fetch_add(total_removed as u64, Ordering::Release);
        self.emit_gc_event(total_removed as u64);
        total_removed
    }

    /// Start the background GC task on the shared thread pool.
    ///
    /// Returns a [`BackgroundTaskHandle`] for the periodic task. The task
    /// runs until `stop()` is called (or [`BackgroundTaskHandle::stop`]).
    pub fn start_background_gc(&self) -> BackgroundTaskHandle {
        let manager = self.clone();
        let running = self.running.clone();
        let interval = Duration::from_millis(self.config.interval_ms);
        let min_interval = Duration::from_millis(self.config.min_interval_between_gc_ms);

        self.pool
            .spawn_periodic(running, interval, min_interval, move || {
                tracing::info!("Vertex GC background task started");

                let start = std::time::Instant::now();

                let removed = manager.run_gc_pass();
                if removed > 0 {
                    tracing::debug!(entries_removed = removed, "Vertex GC pass completed");
                }

                manager.stats.fetch_add(1, Ordering::Release);

                let _elapsed = start.elapsed();
            })
    }

    /// Stop the background GC thread
    pub fn stop(&self) {
        self.running.store(false, Ordering::Release);
    }

    /// Check if the background GC is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Total entries removed since creation
    pub fn total_removed(&self) -> u64 {
        self.total_removed.load(Ordering::Acquire)
    }

    /// Number of GC passes completed
    pub fn pass_count(&self) -> u64 {
        self.stats.load(Ordering::Acquire)
    }
}

impl Clone for VertexGcManager {
    fn clone(&self) -> Self {
        Self {
            data_store: self.data_store.clone(),
            version_manager: self.version_manager.clone(),
            config: self.config.clone(),
            pool: self.pool.clone(),
            running: self.running.clone(),
            stats: AtomicU64::new(self.stats.load(Ordering::Acquire)),
            total_removed: AtomicU64::new(self.total_removed.load(Ordering::Acquire)),
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
}
