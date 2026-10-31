//! Property Graph Configuration

use std::time::Duration;

use crate::compression::CompressionType;
use linkrs_core::error::storage::StorageErrorKind;
use linkrs_core::types::{AutoCompactConfig, Timestamp};
use linkrs_core::StorageError;

pub use crate::engine::freeze_decision::{FreezeDecisionEngine, FreezeDecisionInput};

/// Default number of vertex table shards (hash partitions). Higher shard
/// counts increase write concurrency but widen the internal ID space under
/// imbalanced hash distribution (max ID <= num_shards * live_vertices).
/// The default adapts to the available CPU parallelism, clamped within
/// [1, 256] and rounded up to a power of two (required by shard-ID encoding).
pub fn default_vertex_table_shards() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 256)
        .next_power_of_two()
}

/// Configuration for flush operations.
///
/// The dirty-operation count that arms a flush request lives in
/// [`ResourceConfig::dirty_flush_operations`] so the count and the estimated
/// byte triggers share one owner; only the cadence and codec live here.
#[derive(Debug, Clone)]
pub struct FlushConfig {
    pub flush_interval: Duration,
    pub compression: CompressionType,
}

impl Default for FlushConfig {
    fn default() -> Self {
        Self {
            flush_interval: Duration::from_secs(30),
            compression: CompressionType::Zstd { level: 3 },
        }
    }
}

/// Resource and maintenance limits shared by the storage engine components.
#[derive(Debug, Clone)]
pub struct ResourceConfig {
    /// Hard total memory budget for the storage instance.
    pub max_memory_bytes: u64,
    /// Independent budget reserved for native indexes.
    pub index_memory_bytes: u64,
    /// Ratio at which background maintenance should be scheduled.
    pub memory_soft_ratio: f64,
    /// Ratio at which new work receives a capacity error.
    pub memory_hard_ratio: f64,
    /// Maximum number of active snapshot registrations.
    pub max_active_snapshots: usize,
    /// Maximum age allowed for a snapshot before it is rejected by the owner.
    pub max_snapshot_age: Duration,
    /// Maximum number of MVCC tombstones before backpressure is required.
    pub max_tombstones: usize,
    /// Maximum estimated tombstone memory.
    pub max_tombstone_bytes: u64,
    /// Number of index entries processed by one incremental GC pass.
    pub index_gc_batch: usize,
    /// Maximum duration for one maintenance operation.
    pub operation_timeout: Duration,
    /// Number of dirty operations that triggers a flush request.
    pub dirty_flush_operations: u64,
    /// Estimated dirty bytes that triggers a flush request.
    pub dirty_flush_bytes: u64,
    /// Record cache time-to-live.
    pub cache_ttl: Option<Duration>,
    /// Record cache time-to-idle.
    pub cache_tti: Option<Duration>,
    /// When true, every Moka cache eviction immediately decrements the memory
    /// accounting counter, eliminating accounting lag between refresh intervals.
    pub cache_eviction_sync: bool,
    /// Per-shard native-index buffer pool capacity in bytes.
    pub index_pool_capacity_bytes: u64,
    /// Enable chunk-level eviction under memory pressure.
    pub index_eviction_enabled: bool,
    /// Eviction high-water ratio: trigger eviction when usage/capacity exceeds this.
    pub index_eviction_high_ratio: f64,
    /// Eviction low-water target: evict down to this ratio of capacity.
    pub index_eviction_low_ratio: f64,
    /// Minimum interval between two background-freeze schedules triggered from
    /// the write path. Once the mutable-CSR budget is crossed,
    /// `needs_background_maintenance()` stays true and the per-edge insert hot
    /// path calls `schedule_background_maintenance()` on *every* edge. Without
    /// a cooldown each completed run re-arms immediately and the foreground
    /// writer blocks on the very partition-table lock the background freeze is
    /// holding — a feedback loop that inflates a ~10µs insert to milliseconds.
    /// A single run already makes progress; this interval limits how often the
    /// write path may re-trigger it. Zero disables the throttle.
    pub background_freeze_min_interval: Duration,
    /// Explicit gate for self-starting index/vertex GC loops when reclamation
    /// pressure persists. Off by default so small deployments stay quiet;
    /// observability warnings fire regardless of this gate.
    pub background_gc_auto_start_enabled: bool,
    /// Idle fallback cadence for CSR background maintenance (automatic vertex
    /// compaction, delta freeze, edge auto-maintenance). The write path
    /// already schedules maintenance on pressure; this low-frequency loop
    /// covers writes that paused before crossing a threshold. Dispatch goes
    /// through the shared [`crate::maintenance_scheduler::MaintenanceTask`]
    /// single-flight and cooldown gate, so an idle pass is a no-op.
    /// Zero disables the loop entirely (used by hermetic unit tests).
    pub background_idle_maintenance_interval: Duration,
}

impl Default for ResourceConfig {
    fn default() -> Self {
        Self {
            max_memory_bytes: 512 * 1024 * 1024,
            index_memory_bytes: 128 * 1024 * 1024,
            memory_soft_ratio: 0.80,
            memory_hard_ratio: 0.95,
            max_active_snapshots: 1_000,
            max_snapshot_age: Duration::from_secs(300),
            max_tombstones: 1_000_000,
            max_tombstone_bytes: 256 * 1024 * 1024,
            index_gc_batch: 10_000,
            operation_timeout: Duration::from_secs(30),
            dirty_flush_operations: 50_000,
            dirty_flush_bytes: 64 * 1024 * 1024,
            cache_ttl: Some(Duration::from_secs(300)),
            cache_tti: Some(Duration::from_secs(60)),
            cache_eviction_sync: true,
            index_pool_capacity_bytes: 128 * 1024 * 1024,
            index_eviction_enabled: true,
            index_eviction_high_ratio: 0.85,
            index_eviction_low_ratio: 0.65,
            background_freeze_min_interval: Duration::from_millis(250),
            background_gc_auto_start_enabled: false,
            background_idle_maintenance_interval: Duration::from_secs(30),
        }
    }
}

impl ResourceConfig {
    pub fn validate(&self) -> Result<(), StorageError> {
        crate::engine::resource_budget::MemoryBudget::new(
            self.max_memory_bytes,
            self.index_memory_bytes,
            self.memory_soft_ratio,
            self.memory_hard_ratio,
        )?;
        if self.max_active_snapshots == 0 {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "max_active_snapshots must be greater than 0",
            ));
        }
        if self.max_snapshot_age.is_zero() {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "max_snapshot_age must be greater than 0",
            ));
        }
        if self.max_tombstones == 0 || self.max_tombstone_bytes == 0 {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "tombstone limits must be greater than 0",
            ));
        }
        if self.index_gc_batch == 0 {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "index_gc_batch must be greater than 0",
            ));
        }
        if self.operation_timeout.is_zero() {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "operation_timeout must be greater than 0",
            ));
        }
        if self.dirty_flush_operations == 0 || self.dirty_flush_bytes == 0 {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "dirty flush limits must be greater than 0",
            ));
        }
        for (name, duration) in [("cache_ttl", self.cache_ttl), ("cache_tti", self.cache_tti)] {
            if duration.is_some_and(|value| value.is_zero()) {
                return Err(StorageError::new(
                    StorageErrorKind::InvalidInput,
                    format!("{name} must be greater than 0 when configured"),
                ));
            }
        }
        if let (Some(ttl), Some(tti)) = (self.cache_ttl, self.cache_tti) {
            if tti > ttl {
                return Err(StorageError::new(
                    StorageErrorKind::InvalidInput,
                    "cache_tti must not exceed cache_ttl; otherwise the idle term can never bind",
                ));
            }
        }
        Ok(())
    }
}

/// Unified Freeze Configuration for single-segment CSR compaction.
///
/// Consolidates all freeze-related settings in one place. There are no
/// segments: the mutable CSR size (edge count plus memory) and the deletion
/// ratio drive compaction decisions.
#[derive(Debug, Clone)]
pub struct FreezeConfig {
    // ── Decision Thresholds ──
    /// Freeze when mutable CSR edges exceed this count (out + in entries)
    pub delta_edge_threshold: u64,
    /// Freeze when mutable CSR memory exceeds this (in bytes)
    pub delta_memory_threshold_bytes: u64,
    /// Freeze when the tombstone deletion ratio reaches this (0.0-1.0), so
    /// high-churn tables reclaim physical space without waiting for the
    /// size thresholds above
    pub deletion_threshold: f64,
}

impl FreezeConfig {
    /// Create a conservative configuration (freeze often, merge rarely)
    ///
    /// Suitable for: Development, testing, or when fresh data is critical
    /// - Small freeze threshold (50K edges)
    /// - Very conservative memory usage
    pub fn development() -> Self {
        Self {
            delta_edge_threshold: 50_000,
            delta_memory_threshold_bytes: 128 * 1024 * 1024, // 128MB
            deletion_threshold: 0.5,
        }
    }

    /// Create a production configuration for small systems (< 1M edges)
    ///
    /// Suitable for: Small deployments, single-node systems
    /// - Moderate freeze threshold (100K edges)
    /// - Balanced memory and performance
    pub fn production_small() -> Self {
        Self {
            delta_edge_threshold: 100_000,
            delta_memory_threshold_bytes: 256 * 1024 * 1024, // 256MB
            deletion_threshold: 0.2,
        }
    }

    /// Create a production configuration for large systems (> 1M edges)
    ///
    /// Suitable for: Large deployments, long-running systems
    /// - Large freeze threshold (500K edges)
    /// - Memory threshold stays at 62.5% of the instance soft limit (same
    ///   share as the dev/small presets) so freezing runs before global
    ///   memory pressure degrades the write path
    /// - Optimized for sustained high throughput
    pub fn production_large() -> Self {
        Self {
            delta_edge_threshold: 500_000,
            delta_memory_threshold_bytes: 512 * 1024 * 1024,
            deletion_threshold: 0.3,
        }
    }

    /// Validate configuration for consistency and correctness
    ///
    /// Checks:
    /// - Thresholds are positive
    /// - Deletion ratio is in [0.0, 1.0]
    /// - Memory threshold is reasonable
    pub fn validate(&self) -> Result<(), StorageError> {
        if self.delta_edge_threshold == 0 {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "delta_edge_threshold must be > 0",
            ));
        }

        if self.delta_memory_threshold_bytes == 0 {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "delta_memory_threshold_bytes must be > 0",
            ));
        }

        if !(0.0..=1.0).contains(&self.deletion_threshold) {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                format!(
                    "deletion_threshold must be in [0.0, 1.0], got {}",
                    self.deletion_threshold
                ),
            ));
        }

        Ok(())
    }
}

impl Default for FreezeConfig {
    fn default() -> Self {
        Self::production_small()
    }
}

#[derive(Debug, Clone)]
pub struct PropertyGraphConfig {
    pub enable_cache: bool,
    pub cache_memory: usize,
    pub flush_config: FlushConfig,
    pub resources: ResourceConfig,
    pub freeze: FreezeConfig,
    /// Automatic background vertex compaction (ID hole reclamation)
    pub auto_compact: AutoCompactConfig,
    /// Hash partitions per vertex label table
    pub vertex_table_shards: usize,
    /// Safety margin subtracted from the GC watermark timestamp.
    /// Higher values are more conservative (keep more history).
    pub gc_safety_margin: Timestamp,
    /// Write backpressure seeded into every edge table at creation: max
    /// node-group CSR bytes before background compaction is requested.
    /// Zero disables the trigger. Locked per table at creation time.
    pub edge_max_mutable_csr_bytes: usize,
}

impl Default for PropertyGraphConfig {
    fn default() -> Self {
        Self {
            enable_cache: true,
            cache_memory: 128 * 1024 * 1024,
            flush_config: FlushConfig::default(),
            resources: ResourceConfig::default(),
            freeze: FreezeConfig::default(),
            auto_compact: AutoCompactConfig::default(),
            vertex_table_shards: default_vertex_table_shards(),
            gc_safety_margin: 1,
            edge_max_mutable_csr_bytes: crate::edge::edge_table::config::EdgeTableConfig::default()
                .max_mutable_csr_bytes,
        }
    }
}

impl PropertyGraphConfig {
    /// Create a development configuration
    pub fn development() -> Self {
        let freeze = FreezeConfig::development();
        // Validate configuration on creation for early failure detection
        let _ = freeze.validate();
        Self {
            enable_cache: true,
            cache_memory: 64 * 1024 * 1024, // 64MB for dev
            flush_config: FlushConfig::default(),
            resources: ResourceConfig {
                max_memory_bytes: 256 * 1024 * 1024,
                index_memory_bytes: 64 * 1024 * 1024,
                ..Default::default()
            },
            freeze: freeze.clone(),
            auto_compact: AutoCompactConfig::default(),
            vertex_table_shards: default_vertex_table_shards(),
            gc_safety_margin: 1,
            edge_max_mutable_csr_bytes: crate::edge::edge_table::config::EdgeTableConfig::default()
                .max_mutable_csr_bytes,
        }
    }

    /// Create a production configuration for small systems
    pub fn production_small() -> Self {
        let freeze = FreezeConfig::production_small();
        // Validate configuration on creation for early failure detection
        let _ = freeze.validate();
        Self {
            enable_cache: true,
            cache_memory: 128 * 1024 * 1024,
            flush_config: FlushConfig::default(),
            resources: ResourceConfig::default(),
            freeze: freeze.clone(),
            auto_compact: AutoCompactConfig::default(),
            vertex_table_shards: default_vertex_table_shards(),
            gc_safety_margin: 1,
            edge_max_mutable_csr_bytes: crate::edge::edge_table::config::EdgeTableConfig::default()
                .max_mutable_csr_bytes,
        }
    }

    /// Create a production configuration for large systems
    pub fn production_large() -> Self {
        let freeze = FreezeConfig::production_large();
        // Validate configuration on creation for early failure detection
        let _ = freeze.validate();
        Self {
            enable_cache: true,
            cache_memory: 256 * 1024 * 1024,
            flush_config: FlushConfig::default(),
            resources: ResourceConfig {
                max_memory_bytes: 1024 * 1024 * 1024,
                index_memory_bytes: 256 * 1024 * 1024,
                ..Default::default()
            },
            freeze: freeze.clone(),
            auto_compact: AutoCompactConfig::default(),
            vertex_table_shards: default_vertex_table_shards(),
            gc_safety_margin: 1,
            edge_max_mutable_csr_bytes: crate::edge::edge_table::config::EdgeTableConfig::default()
                .max_mutable_csr_bytes,
        }
    }

    /// Create a lightweight test configuration
    ///
    /// Uses minimal cache (8MB) and relaxed flush thresholds to reduce
    /// resource usage in test environments.
    pub fn test() -> Self {
        Self {
            enable_cache: true,
            cache_memory: 8 * 1024 * 1024,
            flush_config: FlushConfig {
                flush_interval: Duration::from_secs(3600),
                ..Default::default()
            },
            resources: ResourceConfig {
                max_memory_bytes: 64 * 1024 * 1024,
                index_memory_bytes: 16 * 1024 * 1024,
                // Relaxed flush cadence: tests drive persistence explicitly.
                dirty_flush_operations: 100_000,
                // Hermetic unit tests drive maintenance explicitly; no idle
                // loop thread per test context.
                background_idle_maintenance_interval: Duration::ZERO,
                ..Default::default()
            },
            freeze: FreezeConfig {
                delta_edge_threshold: 5000,
                delta_memory_threshold_bytes: 16 * 1024 * 1024,
                deletion_threshold: 0.5,
            },
            auto_compact: AutoCompactConfig::default(),
            vertex_table_shards: default_vertex_table_shards(),
            gc_safety_margin: 1,
            edge_max_mutable_csr_bytes: crate::edge::edge_table::config::EdgeTableConfig::default()
                .max_mutable_csr_bytes,
        }
    }

    /// Validate all configurations
    pub fn validate(&self) -> Result<(), StorageError> {
        if self.enable_cache && self.cache_memory == 0 {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "cache_memory must be > 0 when cache is enabled",
            ));
        }
        self.resources.validate()?;
        if self.enable_cache {
            // Cache must not starve data, index and MVCC: keep it within the
            // memory soft limit instead of the full hard budget.
            let soft_limit_bytes = (self.resources.max_memory_bytes as f64
                * self.resources.memory_soft_ratio)
                .round() as u64;
            if self.cache_memory as u64 > soft_limit_bytes {
                return Err(StorageError::new(
                    StorageErrorKind::InvalidInput,
                    "cache_memory cannot exceed the memory soft limit (max_memory_bytes * memory_soft_ratio)",
                ));
            }
        }
        if self.flush_config.flush_interval.is_zero() {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "flush_interval must be > 0",
            ));
        }
        self.freeze.validate()?;
        // The freeze memory threshold is a per-table budget: keep it below
        // the instance soft limit so one hot table cannot freeze only after
        // global pressure already degraded the write path.
        let soft_limit_bytes = (self.resources.max_memory_bytes as f64
            * self.resources.memory_soft_ratio)
            .round() as u64;
        if self.freeze.delta_memory_threshold_bytes as u64 >= soft_limit_bytes {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "freeze delta_memory_threshold_bytes must stay below the memory soft limit",
            ));
        }
        if !(0.0..=1.0).contains(&self.auto_compact.min_hole_ratio) {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "auto_compact min_hole_ratio must be in [0.0, 1.0]",
            ));
        }
        if !(1..=256).contains(&self.vertex_table_shards)
            || !self.vertex_table_shards.is_power_of_two()
        {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "vertex_table_shards must be a power of two in [1, 256]",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_freeze_config_development() {
        let config = FreezeConfig::development();
        assert_eq!(config.delta_edge_threshold, 50_000);
        assert_eq!(config.delta_memory_threshold_bytes, 128 * 1024 * 1024);
        assert_eq!(config.deletion_threshold, 0.5);
    }

    #[test]
    fn test_freeze_config_production_small() {
        let config = FreezeConfig::production_small();
        assert_eq!(config.delta_edge_threshold, 100_000);
        assert_eq!(config.delta_memory_threshold_bytes, 256 * 1024 * 1024);
        assert_eq!(config.deletion_threshold, 0.2);
    }

    #[test]
    fn test_freeze_config_production_large() {
        let config = FreezeConfig::production_large();
        assert_eq!(config.delta_edge_threshold, 500_000);
        assert_eq!(config.delta_memory_threshold_bytes, 512 * 1024 * 1024);
        assert_eq!(config.deletion_threshold, 0.3);
    }

    #[test]
    fn test_freeze_config_validate_success() {
        let config = FreezeConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_freeze_config_validate_zero_edge_threshold() {
        let config = FreezeConfig {
            delta_edge_threshold: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_freeze_config_validate_zero_memory_threshold() {
        let config = FreezeConfig {
            delta_memory_threshold_bytes: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_freeze_config_validate_invalid_deletion_threshold() {
        let config = FreezeConfig {
            deletion_threshold: 1.5,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        let config = FreezeConfig {
            deletion_threshold: -0.1,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_property_graph_config_development() {
        let config = PropertyGraphConfig::development();
        assert!(config.freeze.validate().is_ok());
    }

    #[test]
    fn test_property_graph_config_production_small() {
        let config = PropertyGraphConfig::production_small();
        assert!(config.freeze.validate().is_ok());
    }

    #[test]
    fn test_property_graph_config_production_large() {
        let config = PropertyGraphConfig::production_large();
        assert!(config.freeze.validate().is_ok());
    }

    #[test]
    fn test_property_graph_config_validate() {
        let config = PropertyGraphConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_resource_config_rejects_invalid_budget_relationships() {
        let config = PropertyGraphConfig {
            resources: ResourceConfig {
                index_memory_bytes: 1024,
                max_memory_bytes: 512,
                ..Default::default()
            },
            ..PropertyGraphConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_resource_config_idle_maintenance_interval_defaults() {
        assert_eq!(
            ResourceConfig::default().background_idle_maintenance_interval,
            Duration::from_secs(30)
        );
        assert!(PropertyGraphConfig::default().validate().is_ok());
        // Hermetic unit tests drive maintenance explicitly: no idle driver.
        assert!(PropertyGraphConfig::test()
            .resources
            .background_idle_maintenance_interval
            .is_zero());
    }
}
