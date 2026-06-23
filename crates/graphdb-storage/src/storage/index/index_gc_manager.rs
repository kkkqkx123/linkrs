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
//! use graphdb::storage::index::{IndexGcManager, IndexGcConfig};
//! use graphdb::transaction::VersionManager;
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

use crate::transaction::VersionManager;
use crate::core::types::Timestamp;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::storage::index::index_data_manager::{GcStats, IndexDataManagerImpl, IndexGcOps};

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
}

/// Index GC Manager
///
/// Manages background garbage collection for index tombstones.
/// Uses incremental GC to avoid blocking normal operations.
pub struct IndexGcManager {
    index_manager: IndexDataManagerImpl,
    version_manager: Arc<VersionManager>,
    config: IndexGcConfig,
    last_gc_ts: AtomicU32,
    running: Arc<AtomicBool>,
    stats: AtomicU64,
    total_removed: AtomicU64,
}

impl IndexGcManager {
    /// Create a new GC manager
    pub fn new(
        index_manager: IndexDataManagerImpl,
        version_manager: Arc<VersionManager>,
        config: IndexGcConfig,
    ) -> Self {
        Self {
            index_manager,
            version_manager,
            config,
            last_gc_ts: AtomicU32::new(0),
            running: Arc::new(AtomicBool::new(false)),
            stats: AtomicU64::new(0),
            total_removed: AtomicU64::new(0),
        }
    }

    /// Run a single GC pass
    ///
    /// Returns the number of entries removed.
    pub fn run_gc_pass(&self) -> GcStats {
        let safe_ts = if self.config.timestamp_margin > 0 {
            self.version_manager
                .get_safe_gc_timestamp_with_margin(self.config.timestamp_margin)
        } else {
            self.version_manager.get_safe_gc_timestamp()
        };

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

        stats
    }

    /// Run aggressive GC until no more tombstones can be removed
    ///
    /// Returns the total number of entries removed.
    pub fn run_aggressive_gc(&self) -> usize {
        let mut total_removed = 0usize;
        let safe_ts = if self.config.timestamp_margin > 0 {
            self.version_manager
                .get_safe_gc_timestamp_with_margin(self.config.timestamp_margin)
        } else {
            self.version_manager.get_safe_gc_timestamp()
        };

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

    /// Start background GC thread
    ///
    /// Returns a JoinHandle for the background thread.
    /// The thread will run until `stop()` is called.
    pub fn start_background_gc(&self) -> JoinHandle<()> {
        let running = self.running.clone();
        let config = self.config.clone();
        let manager = self.clone();

        running.store(true, Ordering::Release);

        thread::spawn(move || {
            tracing::info!("Index GC background thread started");

            while running.load(Ordering::Acquire) {
                let start = std::time::Instant::now();

                if manager.needs_aggressive_gc() {
                    let removed = manager.run_aggressive_gc();
                    if removed > 0 {
                        tracing::debug!(entries_removed = removed, "Aggressive GC completed");
                    }
                } else {
                    let stats = manager.run_gc_pass();
                    if !stats.is_empty() {
                        tracing::debug!(
                            vertex_removed = stats.vertex_entries_removed,
                            "GC pass completed"
                        );
                    }
                }

                manager.stats.fetch_add(1, Ordering::Release);

                let elapsed = start.elapsed();
                let sleep_duration = Duration::from_millis(config.interval_ms)
                    .saturating_sub(elapsed)
                    .max(Duration::from_millis(config.min_interval_between_gc_ms));

                thread::sleep(sleep_duration);
            }

            tracing::info!("Index GC background thread stopped");
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
}

impl Clone for IndexGcManager {
    fn clone(&self) -> Self {
        Self {
            index_manager: self.index_manager.clone(),
            version_manager: self.version_manager.clone(),
            config: self.config.clone(),
            last_gc_ts: AtomicU32::new(self.last_gc_ts.load(Ordering::Acquire)),
            running: self.running.clone(),
            stats: AtomicU64::new(self.stats.load(Ordering::Acquire)),
            total_removed: AtomicU64::new(self.total_removed.load(Ordering::Acquire)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::transaction::VersionManager;
    use crate::storage::index::*;

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
        let gc_manager =
            IndexGcManager::new(index_manager, version_manager, IndexGcConfig::default());

        assert!(!gc_manager.is_running());
        assert_eq!(gc_manager.tombstone_count(), 0);
    }

    #[test]
    fn test_gc_pass_empty() {
        let version_manager = Arc::new(VersionManager::new());
        let index_manager = IndexDataManagerImpl::new();
        let gc_manager =
            IndexGcManager::new(index_manager, version_manager, IndexGcConfig::default());

        let stats = gc_manager.run_gc_pass();
        assert!(stats.is_empty());
    }
}
