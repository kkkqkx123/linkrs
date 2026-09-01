//! Vertex Table Garbage Collection Manager
//!
//! Provides background GC scheduling for vertex table tombstone cleanup.
//! Periodically scans all vertex tables and reclaims space from
//! deleted vertices that are older than the safe GC timestamp.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::engine::data_store::GraphDataStore;
use crate::thread_pool::{BackgroundTaskHandle, StorageThreadPool};
use graphdb_core::types::Timestamp;
use graphdb_transaction::VersionManager;

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
        if let Err(e) = self.data_store.with_vertex_tables_mut(|tables| {
            for table in tables.values() {
                let active = table.active_snapshot_count();
                match table.gc(safe_ts) {
                    Ok(count) => {
                        total_removed += count;
                        if count > 0 && active > 0 {
                            log::debug!(
                                "GC removed {} entries from vertex table with {} active snapshots",
                                count,
                                active,
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

        self.total_removed
            .fetch_add(total_removed as u64, Ordering::Release);
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
