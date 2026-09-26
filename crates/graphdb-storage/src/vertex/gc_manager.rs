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
    /// Upper bound for any single snapshot lease TTL in milliseconds.
    /// The transaction layer negotiates longer holds by renewing, so an
    /// unbounded TTL can never silently pin the watermark forever.
    pub max_lease_ttl_ms: u64,
}

impl Default for VertexGcConfig {
    fn default() -> Self {
        Self {
            interval_ms: 5000,
            min_interval_between_gc_ms: 500,
            timestamp_margin: 1,
            max_lease_ttl_ms: 300_000,
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

    pub fn with_max_lease_ttl(mut self, ttl_ms: u64) -> Self {
        self.max_lease_ttl_ms = ttl_ms.max(1);
        self
    }
}

/// A snapshot lease: the transaction layer declares that `holder` needs
/// reads at or after `floor_ts` until `deadline_ms` (wall clock). Storage
/// never reclaims below a live lease floor; an expired lease stops pinning
/// the watermark and is counted, so holders that overrun their lease are
/// visible instead of silently freezing reclamation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotLease {
    /// Transaction-layer holder id (transaction or cursor id).
    pub holder: u64,
    /// Oldest timestamp the holder may still read.
    pub floor_ts: Timestamp,
    /// Wall-clock expiry in milliseconds since the Unix epoch.
    pub deadline_ms: u64,
}

impl SnapshotLease {
    /// Whether the lease still pins the watermark at `now_ms`.
    pub fn is_live(&self, now_ms: u64) -> bool {
        now_ms < self.deadline_ms
    }
}

/// Point-in-time backpressure view for snapshot admission control.
///
/// Read-only and clock-injected: the transaction layer polls this to decide
/// whether new long-lived snapshots are still admissible. It never kills
/// holders; expiry stays a storage-side reclaim precondition plus an
/// observable counter, never a cross-layer verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotBackpressure {
    /// Live leases pinning the watermark at the sampled instant.
    pub live_leases: usize,
    /// Deepest floor across live leases, if any.
    pub deepest_floor: Option<Timestamp>,
    /// Nearest live-lease deadline in wall-clock milliseconds, if any.
    pub nearest_deadline_ms: Option<u64>,
    /// Leases issued since creation.
    pub leases_issued: u64,
    /// Successful renewals since creation.
    pub leases_renewed: u64,
    /// Expired leases reaped since creation.
    pub leases_expired: u64,
    /// Passes that reclaimed nothing because the watermark was pinned.
    pub blocked_passes: u64,
}

fn wall_now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
    stats: AtomicU64,
    total_removed: AtomicU64,
    /// Passes that reclaimed nothing because the watermark was pinned
    /// (`safe_ts == 0`) or a stuck snapshot held the cutoff. A rising
    /// count beside growing version chains points at the snapshot holder,
    /// not at GC throughput.
    blocked_passes: AtomicU64,
    /// Record-cache label invalidations issued by GC passes that reclaimed
    /// vertex keys. Observed to confirm remap/cache-fence coverage after
    /// churn; a pass that reclaims keys without invalidating is a
    /// correctness bug, not a metric gap.
    cache_invalidations: AtomicU64,
    /// Live snapshot leases by holder id, shared across clones so issuance
    /// on any handle pins every pass. Guarded by a mutex because issuance
    /// is rare (transaction boundaries) while reads take the fast floor
    /// snapshot under the same lock.
    leases: Arc<parking_lot::Mutex<std::collections::HashMap<u64, SnapshotLease>>>,
    /// Leases issued since creation.
    leases_issued: AtomicU64,
    /// Successful lease renewals since creation.
    leases_renewed: AtomicU64,
    /// Leases found expired and reaped since creation.
    leases_expired: AtomicU64,
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
            blocked_passes: AtomicU64::new(0),
            cache_invalidations: AtomicU64::new(0),
            leases: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
            leases_issued: AtomicU64::new(0),
            leases_renewed: AtomicU64::new(0),
            leases_expired: AtomicU64::new(0),
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

    /// Issue (or replace) a snapshot lease for `holder` starting at
    /// `now_ms` with the requested TTL, clamped to the configured maximum.
    /// `now_ms` is a parameter (not read from the clock) so expiry is
    /// unit-testable without time mocking; production passes
    /// wall-clock time.
    // Lease issuance is driven by the transaction layer; storage only
    // reaps expired leases and floors the cutoff. Retained as the
    // cross-layer interface; exercised by the lease tests below.
    #[allow(dead_code)]
    pub fn issue_lease(
        &self,
        holder: u64,
        floor_ts: Timestamp,
        ttl_ms: u64,
        now_ms: u64,
    ) -> SnapshotLease {
        let ttl = ttl_ms.min(self.config.max_lease_ttl_ms).max(1);
        let lease = SnapshotLease {
            holder,
            floor_ts,
            deadline_ms: now_ms.saturating_add(ttl),
        };
        self.leases.lock().insert(holder, lease);
        self.leases_issued.fetch_add(1, Ordering::Release);
        lease
    }

    /// Extend `holder`'s lease by `ttl_ms` from `now_ms`. Returns false
    /// when no lease exists (the holder must issue rather than renew).
    // See the issuance note above: transaction-layer driven.
    #[allow(dead_code)]
    pub fn renew_lease(&self, holder: u64, ttl_ms: u64, now_ms: u64) -> bool {
        let ttl = ttl_ms.min(self.config.max_lease_ttl_ms).max(1);
        let mut leases = self.leases.lock();
        match leases.get_mut(&holder) {
            Some(lease) => {
                lease.deadline_ms = now_ms.saturating_add(ttl);
                self.leases_renewed.fetch_add(1, Ordering::Release);
                true
            }
            None => false,
        }
    }

    /// Drop `holder`'s lease. Returns false when none existed.
    // See the issuance note above: transaction-layer driven.
    #[allow(dead_code)]
    pub fn release_lease(&self, holder: u64) -> bool {
        self.leases.lock().remove(&holder).is_some()
    }

    /// Remove leases expired at `now_ms`, counting each one. Expired
    /// leases stop pinning the watermark; the count keeps overrunning
    /// holders observable.
    pub fn reap_expired_leases(&self, now_ms: u64) -> usize {
        let mut leases = self.leases.lock();
        let before = leases.len();
        leases.retain(|_, lease| lease.is_live(now_ms));
        let reaped = before - leases.len();
        if reaped > 0 {
            self.leases_expired
                .fetch_add(reaped as u64, Ordering::Release);
        }
        reaped
    }

    /// Deepest (minimum) floor across live leases, if any. Leases only pin
    /// downward: the GC cutoff is the minimum of the version-manager safe
    /// timestamp and this floor, never above it.
    pub fn lease_floor(&self, now_ms: u64) -> Option<Timestamp> {
        self.leases
            .lock()
            .values()
            .filter(|lease| lease.is_live(now_ms))
            .map(|lease| lease.floor_ts)
            .min()
    }

    /// Capture the backpressure view in one lock hold: live-lease pressure
    /// plus the counters a reviewer crosses against `blocked_passes` to
    /// tell a stuck snapshot from idle GC.
    pub fn backpressure_snapshot(&self, now_ms: u64) -> SnapshotBackpressure {
        let leases = self.leases.lock();
        let mut live_leases = 0usize;
        let mut deepest_floor: Option<Timestamp> = None;
        let mut nearest_deadline_ms: Option<u64> = None;
        for lease in leases.values().filter(|lease| lease.is_live(now_ms)) {
            live_leases += 1;
            deepest_floor = Some(match deepest_floor {
                Some(floor) => floor.min(lease.floor_ts),
                None => lease.floor_ts,
            });
            nearest_deadline_ms = Some(match nearest_deadline_ms {
                Some(deadline) => deadline.min(lease.deadline_ms),
                None => lease.deadline_ms,
            });
        }
        SnapshotBackpressure {
            live_leases,
            deepest_floor,
            nearest_deadline_ms,
            leases_issued: self.leases_issued.load(Ordering::Acquire),
            leases_renewed: self.leases_renewed.load(Ordering::Acquire),
            leases_expired: self.leases_expired.load(Ordering::Acquire),
            blocked_passes: self.blocked_passes.load(Ordering::Acquire),
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

        // Snapshot leases pin the cutoff downward only: a live lease floor
        // below the version-manager safe timestamp holds reclamation back,
        // and expired leases are reaped first so overrunning holders cannot
        // freeze the watermark. No leases means no behavior change.
        self.reap_expired_leases(wall_now_ms());
        let cutoff = match self.lease_floor(wall_now_ms()) {
            Some(floor) => safe_ts.min(floor),
            None => safe_ts,
        };

        if cutoff == 0 {
            self.blocked_passes.fetch_add(1, Ordering::Release);
            return 0;
        }
        if watermarks.has_active_snapshot() {
            if let Some(age) = watermarks.oldest_age(&self.version_manager) {
                if is_snapshot_stuck(age.as_secs()) {
                    self.blocked_passes.fetch_add(1, Ordering::Release);
                    // Surface the lease backpressure view beside the stuck
                    // warning so reviewers can cross-check blocked passes
                    // against lease issuance, renewal, and expiry counts.
                    let backpressure = self.backpressure_snapshot(wall_now_ms());
                    log::warn!(
                        "GC blocked by long-lived snapshot age={:?} safe_gc={} oldest_active={} live_leases={} leases_issued={} leases_renewed={} leases_expired={} blocked_passes={}",
                        age,
                        safe_ts,
                        watermarks.oldest_active_snapshot,
                        backpressure.live_leases,
                        backpressure.leases_issued,
                        backpressure.leases_renewed,
                        backpressure.leases_expired,
                        backpressure.blocked_passes,
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
            blocked_passes: AtomicU64::new(self.blocked_passes.load(Ordering::Acquire)),
            cache_invalidations: AtomicU64::new(self.cache_invalidations.load(Ordering::Acquire)),
            leases: self.leases.clone(),
            leases_issued: AtomicU64::new(self.leases_issued.load(Ordering::Acquire)),
            leases_renewed: AtomicU64::new(self.leases_renewed.load(Ordering::Acquire)),
            leases_expired: AtomicU64::new(self.leases_expired.load(Ordering::Acquire)),
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
        assert_eq!(gc.backpressure_snapshot(0).blocked_passes, 0);
    }

    #[test]
    fn test_stuck_snapshot_threshold() {
        assert!(!is_snapshot_stuck(0));
        assert!(!is_snapshot_stuck(STUCK_SNAPSHOT_AGE_SECS));
        assert!(is_snapshot_stuck(STUCK_SNAPSHOT_AGE_SECS + 1));
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
    fn test_lease_lifecycle_pins_floor_downward_only() {
        let gc = test_manager();
        assert_eq!(gc.lease_floor(1_000), None);
        let lease = gc.issue_lease(7, 500, 10_000, 1_000);
        assert_eq!(lease.deadline_ms, 11_000);
        assert_eq!(gc.backpressure_snapshot(2_000).leases_issued, 1);
        assert_eq!(gc.lease_floor(2_000), Some(500));
        assert!(gc.renew_lease(7, 10_000, 2_000));
        assert_eq!(gc.lease_floor(11_500), Some(500));
        assert!(gc.release_lease(7));
        assert!(!gc.release_lease(7));
        assert_eq!(gc.lease_floor(11_500), None);
        assert!(!gc.renew_lease(7, 10_000, 11_500));
    }

    #[test]
    fn test_lease_ttl_clamped_to_config_maximum() {
        let gc = test_manager();
        let lease = gc.issue_lease(1, 100, u64::MAX, 0);
        assert_eq!(
            lease.deadline_ms,
            VertexGcConfig::default().max_lease_ttl_ms
        );
    }

    #[test]
    fn test_lease_renewals_counted_and_backpressure_snapshot() {
        let gc = test_manager();
        gc.issue_lease(1, 100, 1_000, 0);
        gc.issue_lease(2, 50, 5_000, 0);
        assert!(gc.renew_lease(1, 2_000, 500));
        assert!(gc.renew_lease(1, 2_000, 600));
        assert!(!gc.renew_lease(9, 1_000, 600));
        assert_eq!(gc.backpressure_snapshot(600).leases_renewed, 2);
        assert_eq!(gc.clone().backpressure_snapshot(600).leases_renewed, 2);
        let backpressure = gc.backpressure_snapshot(700);
        assert_eq!(backpressure.live_leases, 2);
        assert_eq!(backpressure.deepest_floor, Some(50));
        assert_eq!(backpressure.nearest_deadline_ms, Some(2_600));
        assert_eq!(backpressure.leases_issued, 2);
        assert_eq!(backpressure.leases_renewed, 2);
        assert_eq!(backpressure.leases_expired, 0);
        assert_eq!(backpressure.blocked_passes, 0);
        let aged = gc.backpressure_snapshot(6_000);
        assert_eq!(aged.live_leases, 0);
        assert_eq!(aged.deepest_floor, None);
        assert_eq!(aged.nearest_deadline_ms, None);
    }

    #[test]
    fn test_expired_leases_reaped_and_counted() {
        let gc = test_manager();
        gc.issue_lease(1, 100, 1_000, 0);
        gc.issue_lease(2, 50, 1_000, 0);
        gc.issue_lease(3, 300, 60_000, 0);
        assert_eq!(gc.lease_floor(500), Some(50));
        assert_eq!(gc.reap_expired_leases(2_000), 2);
        assert_eq!(gc.backpressure_snapshot(2_000).leases_expired, 2);
        assert_eq!(gc.lease_floor(2_000), Some(300));
        assert_eq!(gc.reap_expired_leases(70_000), 1);
        assert_eq!(gc.lease_floor(70_000), None);
    }
}
