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

use crate::core::types::Timestamp;
use crate::transaction::VersionManager;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::storage::index::traits::IndexGcOps;
use crate::storage::index::types::{GcStats, IndexIdentity};
use crate::storage::index::IndexDataManagerImpl;

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
    last_gc_ts: AtomicU64,
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
            last_gc_ts: AtomicU64::new(0),
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
            last_gc_ts: AtomicU64::new(self.last_gc_ts.load(Ordering::Acquire)),
            running: self.running.clone(),
            stats: AtomicU64::new(self.stats.load(Ordering::Acquire)),
            total_removed: AtomicU64::new(self.total_removed.load(Ordering::Acquire)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::storage::index::*;
    use crate::transaction::VersionManager;

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
