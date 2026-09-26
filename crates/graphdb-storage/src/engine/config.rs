//! Property Graph Configuration

use std::time::Duration;

use crate::compression::CompressionType;
use graphdb_core::error::storage::StorageErrorKind;
use graphdb_core::types::{AutoCompactConfig, Timestamp};
use graphdb_core::StorageError;

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

/// Configuration for flush operations
#[derive(Debug, Clone)]
pub struct FlushConfig {
    pub flush_threshold: usize,
    pub flush_interval: Duration,
    pub compression: CompressionType,
}

impl Default for FlushConfig {
    fn default() -> Self {
        Self {
            flush_threshold: 50_000,
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
    /// Ratio of hard_limit at which the disk spiller proactively evicts cold data.
    /// Must satisfy soft_ratio < spill_threshold_ratio <= 1.0.
    pub spill_threshold_ratio: f64,
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
            cache_ttl: Some(Duration::from_secs(60)),
            cache_tti: Some(Duration::from_secs(300)),
            spill_threshold_ratio: 0.90,
            cache_eviction_sync: true,
            index_pool_capacity_bytes: 128 * 1024 * 1024,
            index_eviction_enabled: true,
            index_eviction_high_ratio: 0.85,
            index_eviction_low_ratio: 0.65,
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
        if !self.spill_threshold_ratio.is_finite()
            || self.spill_threshold_ratio <= self.memory_soft_ratio
            || self.spill_threshold_ratio > 1.0
        {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                format!(
                    "spill_threshold_ratio ({}) must satisfy soft_ratio < spill_ratio <= 1.0",
                    self.spill_threshold_ratio
                ),
            ));
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
    /// - Optimized for sustained high throughput
    pub fn production_large() -> Self {
        Self {
            delta_edge_threshold: 500_000,
            delta_memory_threshold_bytes: 1_000_000_000, // 1GB
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
                flush_threshold: 100000,
                flush_interval: Duration::from_secs(3600),
                ..Default::default()
            },
            resources: ResourceConfig {
                max_memory_bytes: 64 * 1024 * 1024,
                index_memory_bytes: 16 * 1024 * 1024,
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
        if self.enable_cache && self.cache_memory as u64 > self.resources.max_memory_bytes {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "cache_memory cannot exceed max_memory_bytes",
            ));
        }
        self.resources.validate()?;
        if self.flush_config.flush_threshold == 0 {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "flush_threshold must be > 0",
            ));
        }
        if self.flush_config.flush_interval.is_zero() {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "flush_interval must be > 0",
            ));
        }
        self.freeze.validate()?;
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
        assert_eq!(config.delta_memory_threshold_bytes, 1_000_000_000);
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
}
