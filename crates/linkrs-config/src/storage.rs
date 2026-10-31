//! Storage configuration

use serde::{Deserialize, Serialize};

use crate::runtime::SyncMode;

/// Storage engine type
#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StorageEngine {
    /// PropertyGraph storage engine (columnar + CSR)
    #[default]
    PropertyGraph,
    /// RocksDB storage engine (future support)
    #[serde(rename = "rocksdb")]
    RocksDB,
}

impl std::fmt::Display for StorageEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PropertyGraph => write!(f, "propertygraph"),
            Self::RocksDB => write!(f, "rocksdb"),
        }
    }
}

/// Compression algorithm
#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CompressionAlgorithm {
    /// No compression
    #[default]
    None,
    /// Zstandard compression
    Zstd,
}

impl std::fmt::Display for CompressionAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Zstd => write!(f, "zstd"),
        }
    }
}

/// Storage configuration
///
/// Configures the storage engine behavior and performance characteristics.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    /// Storage engine type (propertygraph, rocksdb, etc.)
    #[serde(default)]
    pub engine: StorageEngine,

    /// Compression algorithm (none, lz4, zstd, snappy)
    #[serde(default)]
    pub compression: CompressionAlgorithm,

    /// Compression level (0-9, engine-dependent)
    #[serde(default = "default_compression_level")]
    pub compression_level: u32,

    /// Checkpoint interval (seconds, 0 = disabled)
    #[serde(default = "default_checkpoint_interval")]
    pub checkpoint_interval_secs: u64,

    /// WAL synchronization policy applied at storage startup.
    #[serde(default = "default_sync_mode")]
    pub sync_mode: SyncMode,

    /// Maximum database size (bytes, 0 = unlimited)
    #[serde(default)]
    pub max_db_size: u64,

    /// Enable automatic statistics collection
    #[serde(default = "default_true")]
    pub auto_statistics: bool,

    /// Statistics collection interval (seconds)
    #[serde(default = "default_statistics_interval")]
    pub statistics_interval_secs: u64,

    /// Total storage memory budget in bytes.
    #[serde(default = "default_max_memory_bytes")]
    pub max_memory_bytes: u64,

    /// Independent native-index memory budget in bytes.
    #[serde(default = "default_index_memory_bytes")]
    pub index_memory_bytes: u64,

    /// Soft memory pressure ratio.
    #[serde(default = "default_memory_soft_ratio")]
    pub memory_soft_ratio: f64,

    /// Hard memory admission ratio.
    #[serde(default = "default_memory_hard_ratio")]
    pub memory_hard_ratio: f64,

    /// Maximum number of active snapshots.
    #[serde(default = "default_max_active_snapshots")]
    pub max_active_snapshots: usize,

    /// Maximum snapshot age in seconds.
    #[serde(default = "default_max_snapshot_age_secs")]
    pub max_snapshot_age_secs: u64,

    /// Maximum number of retained tombstones.
    #[serde(default = "default_max_tombstones")]
    pub max_tombstones: usize,

    /// Maximum estimated tombstone memory in bytes.
    #[serde(default = "default_max_tombstone_bytes")]
    pub max_tombstone_bytes: u64,

    /// Number of index entries processed per GC pass.
    #[serde(default = "default_index_gc_batch")]
    pub index_gc_batch: usize,

    /// Enable the background sweep that physically reclaims TTL-expired rows.
    ///
    /// Off by default: expired rows stay visible only to reads that filter
    /// them until `PURGE EXPIRED` runs (explicitly, or via this sweep once
    /// enabled).
    #[serde(default = "default_false")]
    pub ttl_purge_enabled: bool,

    /// Background TTL purge interval in seconds. Zero never starts the sweep,
    /// as does leaving `ttl_purge_enabled` off.
    #[serde(default = "default_ttl_purge_interval")]
    pub ttl_purge_interval_secs: u64,

    /// Enable the periodic index GC background loop. Off by default:
    /// tombstone reclamation then runs only through the opportunistic
    /// write-path trigger. Read-heavy deployments with few writes should
    /// enable this so generation retirement stays bounded without write
    /// traffic.
    #[serde(default = "default_false")]
    pub index_gc_background_enabled: bool,

    /// Enable the periodic vertex GC background loop. Off by default:
    /// deleted-vertex reclamation then never runs on its own. Enable this
    /// when deletes run without steady write pressure to drive the loop.
    #[serde(default = "default_false")]
    pub vertex_gc_background_enabled: bool,

    /// Explicit gate for self-starting GC loops on sustained pressure.
    /// Off by default so small deployments stay quiet. Pressure warnings
    /// fire regardless of this gate.
    #[serde(default = "default_false")]
    pub background_gc_auto_start_enabled: bool,

    /// Idle fallback cadence for CSR background maintenance in seconds.
    /// Zero disables the loop entirely. Defaults to 30.
    #[serde(default = "default_background_idle_maintenance_secs")]
    pub background_idle_maintenance_secs: u64,

    /// Create one final checkpoint when the storage is closed explicitly.
    /// Off by default so close stays fast.
    #[serde(default = "default_false")]
    pub checkpoint_on_close: bool,

    /// Maximum maintenance operation duration in seconds.
    #[serde(default = "default_operation_timeout_secs")]
    pub operation_timeout_secs: u64,

    /// Number of dirty operations that triggers a flush request.
    #[serde(default = "default_dirty_flush_operations")]
    pub dirty_flush_operations: u64,

    /// Estimated dirty bytes that triggers a flush request.
    #[serde(default = "default_dirty_flush_bytes")]
    pub dirty_flush_bytes: u64,

    /// Record cache memory budget in bytes. Must stay within the instance
    /// memory soft limit so data, index and MVCC keep their share of the
    /// budget. Build-time only: changing it requires recreating the cache.
    #[serde(default = "default_cache_memory_bytes")]
    pub cache_memory_bytes: u64,

    /// Record cache TTL in seconds. Zero disables TTL.
    /// When both TTL and TTI are set, TTI must not exceed TTL — otherwise
    /// the idle term can never bind and the knob is dead.
    #[serde(default = "default_cache_ttl_secs")]
    pub cache_ttl_secs: u64,

    /// Record cache TTI in seconds. Zero disables TTI.
    #[serde(default = "default_cache_tti_secs")]
    pub cache_tti_secs: u64,

    /// Minimum number of rows required for string encoding analysis.
    #[serde(default = "default_string_min_rows")]
    pub string_min_rows: usize,

    /// Minimum average string length to consider FSST encoding.
    #[serde(default = "default_avg_length_threshold")]
    pub avg_length_threshold: usize,

    /// Cardinality ratio (distinct / total) below which Dictionary is preferred.
    #[serde(default = "default_cardinality_ratio_threshold")]
    pub cardinality_ratio_threshold: f64,

    /// Ratio of new data to existing data that triggers FSST rebuild.
    #[serde(default = "default_fsst_rebuild_threshold")]
    pub fsst_rebuild_threshold: f64,

    /// Per-shard native-index buffer pool capacity in bytes.
    #[serde(default = "default_index_pool_capacity_bytes")]
    pub index_pool_capacity_bytes: u64,

    /// Enable chunk-level eviction under memory pressure.
    #[serde(default = "default_true")]
    pub index_eviction_enabled: bool,

    /// Eviction high-water ratio: trigger eviction when usage/capacity exceeds this.
    #[serde(default = "default_index_eviction_high_ratio")]
    pub index_eviction_high_ratio: f64,

    /// Eviction low-water target: evict down to this ratio of capacity.
    #[serde(default = "default_index_eviction_low_ratio")]
    pub index_eviction_low_ratio: f64,

    /// Dedicated checkpoint execution threads. One driver plus this lane
    /// stays decoupled from the shared compute pool. Defaults to one.
    #[serde(default = "default_checkpoint_executor_threads")]
    pub checkpoint_executor_threads: usize,

    /// Freeze when mutable CSR edges exceed this count (out + in entries).
    #[serde(default = "default_freeze_delta_edge_threshold")]
    pub freeze_delta_edge_threshold: u64,

    /// Freeze when mutable CSR memory exceeds this in bytes. Must stay below
    /// the memory soft limit (max_memory_bytes * memory_soft_ratio) so one hot
    /// table freezes before global memory pressure degrades the write path.
    #[serde(default = "default_freeze_delta_memory_threshold_bytes")]
    pub freeze_delta_memory_threshold_bytes: u64,

    /// Freeze when the tombstone deletion ratio reaches this (0.0-1.0).
    #[serde(default = "default_freeze_deletion_threshold")]
    pub freeze_deletion_threshold: f64,

    /// Minimum absolute number of holes (allocated minus live IDs) before
    /// automatic vertex compaction runs.
    #[serde(default = "default_auto_compact_min_holes")]
    pub auto_compact_min_holes: u32,

    /// Minimum hole ratio (holes / allocated IDs) before automatic vertex
    /// compaction runs. Range [0.0, 1.0].
    #[serde(default = "default_auto_compact_min_hole_ratio")]
    pub auto_compact_min_hole_ratio: f64,

    /// Cooldown between automatic vertex compactions, in seconds.
    #[serde(default = "default_auto_compact_min_interval_secs")]
    pub auto_compact_min_interval_secs: u64,

    /// Write backpressure seeded into every edge table at creation: max
    /// node-group CSR bytes before background compaction is requested. Zero
    /// disables the trigger. Locked per table at creation time.
    #[serde(default = "default_edge_max_mutable_csr_bytes")]
    pub edge_max_mutable_csr_bytes: u64,
}

fn default_compression_level() -> u32 {
    3
}

fn default_checkpoint_interval() -> u64 {
    300 // 5 minutes
}

fn default_statistics_interval() -> u64 {
    60 // 1 minute
}

fn default_true() -> bool {
    true
}

fn default_max_memory_bytes() -> u64 {
    512 * 1024 * 1024
}

fn default_index_memory_bytes() -> u64 {
    128 * 1024 * 1024
}

fn default_memory_soft_ratio() -> f64 {
    0.80
}

fn default_memory_hard_ratio() -> f64 {
    0.95
}

fn default_max_active_snapshots() -> usize {
    1_000
}

fn default_max_snapshot_age_secs() -> u64 {
    300
}

fn default_max_tombstones() -> usize {
    1_000_000
}

fn default_max_tombstone_bytes() -> u64 {
    256 * 1024 * 1024
}

fn default_index_gc_batch() -> usize {
    10_000
}

fn default_false() -> bool {
    false
}

fn default_ttl_purge_interval() -> u64 {
    3600 // 1 hour
}

fn default_operation_timeout_secs() -> u64 {
    30
}

fn default_dirty_flush_operations() -> u64 {
    50_000
}

fn default_dirty_flush_bytes() -> u64 {
    64 * 1024 * 1024
}

fn default_cache_memory_bytes() -> u64 {
    128 * 1024 * 1024
}

fn default_cache_ttl_secs() -> u64 {
    300
}

fn default_cache_tti_secs() -> u64 {
    60
}

fn default_string_min_rows() -> usize {
    50
}

fn default_avg_length_threshold() -> usize {
    16
}

fn default_cardinality_ratio_threshold() -> f64 {
    0.5
}

fn default_fsst_rebuild_threshold() -> f64 {
    0.2
}

fn default_index_pool_capacity_bytes() -> u64 {
    128 * 1024 * 1024
}

fn default_index_eviction_high_ratio() -> f64 {
    0.85
}

fn default_index_eviction_low_ratio() -> f64 {
    0.65
}

fn default_checkpoint_executor_threads() -> usize {
    1
}

fn default_freeze_delta_edge_threshold() -> u64 {
    100_000
}

fn default_freeze_delta_memory_threshold_bytes() -> u64 {
    256 * 1024 * 1024
}

fn default_freeze_deletion_threshold() -> f64 {
    0.2
}

fn default_auto_compact_min_holes() -> u32 {
    65_536
}

fn default_auto_compact_min_hole_ratio() -> f64 {
    0.25
}

fn default_auto_compact_min_interval_secs() -> u64 {
    3600
}

fn default_edge_max_mutable_csr_bytes() -> u64 {
    100 * 1024 * 1024
}

fn default_background_idle_maintenance_secs() -> u64 {
    30
}

fn default_sync_mode() -> SyncMode {
    SyncMode::Full
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            engine: StorageEngine::default(),
            compression: CompressionAlgorithm::default(),
            compression_level: default_compression_level(),
            checkpoint_interval_secs: default_checkpoint_interval(),
            sync_mode: default_sync_mode(),
            max_db_size: 0, // Unlimited
            auto_statistics: true,
            statistics_interval_secs: default_statistics_interval(),
            max_memory_bytes: default_max_memory_bytes(),
            index_memory_bytes: default_index_memory_bytes(),
            memory_soft_ratio: default_memory_soft_ratio(),
            memory_hard_ratio: default_memory_hard_ratio(),
            max_active_snapshots: default_max_active_snapshots(),
            max_snapshot_age_secs: default_max_snapshot_age_secs(),
            max_tombstones: default_max_tombstones(),
            max_tombstone_bytes: default_max_tombstone_bytes(),
            index_gc_batch: default_index_gc_batch(),
            ttl_purge_enabled: default_false(),
            ttl_purge_interval_secs: default_ttl_purge_interval(),
            index_gc_background_enabled: default_false(),
            vertex_gc_background_enabled: default_false(),
            background_gc_auto_start_enabled: default_false(),
            background_idle_maintenance_secs: default_background_idle_maintenance_secs(),
            checkpoint_on_close: default_false(),
            operation_timeout_secs: default_operation_timeout_secs(),
            dirty_flush_operations: default_dirty_flush_operations(),
            dirty_flush_bytes: default_dirty_flush_bytes(),
            cache_ttl_secs: default_cache_ttl_secs(),
            cache_tti_secs: default_cache_tti_secs(),
            cache_memory_bytes: default_cache_memory_bytes(),
            string_min_rows: default_string_min_rows(),
            avg_length_threshold: default_avg_length_threshold(),
            cardinality_ratio_threshold: default_cardinality_ratio_threshold(),
            fsst_rebuild_threshold: default_fsst_rebuild_threshold(),
            index_pool_capacity_bytes: default_index_pool_capacity_bytes(),
            index_eviction_enabled: true,
            index_eviction_high_ratio: default_index_eviction_high_ratio(),
            index_eviction_low_ratio: default_index_eviction_low_ratio(),
            checkpoint_executor_threads: default_checkpoint_executor_threads(),
            freeze_delta_edge_threshold: default_freeze_delta_edge_threshold(),
            freeze_delta_memory_threshold_bytes: default_freeze_delta_memory_threshold_bytes(),
            freeze_deletion_threshold: default_freeze_deletion_threshold(),
            auto_compact_min_holes: default_auto_compact_min_holes(),
            auto_compact_min_hole_ratio: default_auto_compact_min_hole_ratio(),
            auto_compact_min_interval_secs: default_auto_compact_min_interval_secs(),
            edge_max_mutable_csr_bytes: default_edge_max_mutable_csr_bytes(),
        }
    }
}

impl StorageConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.compression_level > 9 {
            return Err("Compression level must be between 0 and 9".to_string());
        }
        if self.max_memory_bytes == 0
            || self.index_memory_bytes == 0
            || self.index_memory_bytes > self.max_memory_bytes
        {
            return Err(
                "memory budgets must be positive and index_memory_bytes cannot exceed max_memory_bytes"
                    .to_string(),
            );
        }
        if !self.memory_soft_ratio.is_finite()
            || !self.memory_hard_ratio.is_finite()
            || !(0.0..=1.0).contains(&self.memory_soft_ratio)
            || !(0.0..=1.0).contains(&self.memory_hard_ratio)
            || self.memory_soft_ratio == 0.0
            || self.memory_soft_ratio >= self.memory_hard_ratio
        {
            return Err("memory ratios must satisfy 0 < soft < hard <= 1".to_string());
        }
        if self.max_active_snapshots == 0
            || self.max_snapshot_age_secs == 0
            || self.max_tombstones == 0
            || self.max_tombstone_bytes == 0
            || self.index_gc_batch == 0
            || self.operation_timeout_secs == 0
            || self.dirty_flush_operations == 0
            || self.dirty_flush_bytes == 0
            || self.index_pool_capacity_bytes == 0
        {
            return Err("storage resource limits must be greater than 0".to_string());
        }
        if !self.index_eviction_high_ratio.is_finite()
            || !self.index_eviction_low_ratio.is_finite()
            || !(0.0..=1.0).contains(&self.index_eviction_high_ratio)
            || !(0.0..=1.0).contains(&self.index_eviction_low_ratio)
            || self.index_eviction_low_ratio >= self.index_eviction_high_ratio
        {
            return Err("index eviction ratios must satisfy 0 < low < high <= 1".to_string());
        }
        if self.cache_ttl_secs > 0 && self.cache_tti_secs > 0 && self.cache_tti_secs > self.cache_ttl_secs
        {
            return Err(
                "cache_tti_secs must not exceed cache_ttl_secs; otherwise the idle term can never bind"
                    .to_string(),
            );
        }
        if self.cache_memory_bytes == 0 {
            return Err("cache_memory_bytes must be greater than 0".to_string());
        }
        let memory_soft_limit = (self.max_memory_bytes as f64 * self.memory_soft_ratio).round() as u64;
        if self.cache_memory_bytes > memory_soft_limit {
            return Err(
                "cache_memory_bytes must not exceed the memory soft limit (max_memory_bytes * memory_soft_ratio)"
                    .to_string(),
            );
        }
        if self.checkpoint_executor_threads == 0 {
            return Err("checkpoint_executor_threads must be greater than 0".to_string());
        }
        if self.freeze_delta_edge_threshold == 0 || self.freeze_delta_memory_threshold_bytes == 0 {
            return Err("freeze thresholds must be greater than 0".to_string());
        }
        if !self.freeze_deletion_threshold.is_finite()
            || !(0.0..=1.0).contains(&self.freeze_deletion_threshold)
        {
            return Err("freeze_deletion_threshold must be in [0.0, 1.0]".to_string());
        }
        if !self.auto_compact_min_hole_ratio.is_finite()
            || !(0.0..=1.0).contains(&self.auto_compact_min_hole_ratio)
        {
            return Err("auto_compact_min_hole_ratio must be in [0.0, 1.0]".to_string());
        }
        let freeze_soft_limit =
            (self.max_memory_bytes as f64 * self.memory_soft_ratio).round() as u64;
        if self.freeze_delta_memory_threshold_bytes >= freeze_soft_limit {
            return Err(
                "freeze_delta_memory_threshold_bytes must stay below the memory soft limit (max_memory_bytes * memory_soft_ratio)"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// Check if compression is enabled
    pub fn is_compression_enabled(&self) -> bool {
        !matches!(self.compression, CompressionAlgorithm::None)
    }
}

/// Query resource configuration
///
/// Controls resource limits for query execution.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct QueryResourceConfig {
    /// Maximum memory per query (bytes, 0 = unlimited)
    #[serde(default)]
    pub max_memory_per_query: u64,

    /// Maximum concurrent queries (0 = unlimited)
    #[serde(default = "default_max_concurrent_queries")]
    pub max_concurrent_queries: usize,

    /// Query timeout (seconds, 0 = no timeout)
    #[serde(default)]
    pub query_timeout_secs: u64,

    /// Maximum result set size (0 = unlimited)
    #[serde(default)]
    pub max_result_size: usize,

    /// Maximum number of vertices to scan in a single query
    #[serde(default = "default_max_vertex_scan")]
    pub max_vertex_scan: usize,

    /// Maximum number of edges to scan in a single query
    #[serde(default = "default_max_edge_scan")]
    pub max_edge_scan: usize,
}

fn default_max_concurrent_queries() -> usize {
    100
}

fn default_max_vertex_scan() -> usize {
    1_000_000
}

fn default_max_edge_scan() -> usize {
    10_000_000
}

impl Default for QueryResourceConfig {
    fn default() -> Self {
        Self {
            max_memory_per_query: 0, // Unlimited
            max_concurrent_queries: default_max_concurrent_queries(),
            query_timeout_secs: 0, // No timeout
            max_result_size: 0,    // Unlimited
            max_vertex_scan: default_max_vertex_scan(),
            max_edge_scan: default_max_edge_scan(),
        }
    }
}

impl QueryResourceConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.max_concurrent_queries == 0 {
            return Err("Max concurrent queries must be greater than 0".to_string());
        }

        Ok(())
    }

    /// Check if memory limit is enabled
    pub fn has_memory_limit(&self) -> bool {
        self.max_memory_per_query > 0
    }

    /// Check if query timeout is enabled
    pub fn has_timeout(&self) -> bool {
        self.query_timeout_secs > 0
    }

    /// Check if result size limit is enabled
    pub fn has_result_size_limit(&self) -> bool {
        self.max_result_size > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_config_default() {
        let config = StorageConfig::default();
        assert_eq!(config.engine, StorageEngine::PropertyGraph);
        assert_eq!(config.compression, CompressionAlgorithm::None);
        assert_eq!(config.compression_level, 3);
        assert_eq!(config.checkpoint_interval_secs, 300);
        assert_eq!(config.sync_mode, SyncMode::Full);
        assert!(config.auto_statistics);
        // The TTL purge sweep is opt-in: disabled with a positive interval,
        // so a zero interval is the only other way it stays off. GC
        // background loops are opt-in too: the default is opportunistic-only
        // (index GC on the write path, vertex GC idle).
        assert!(!config.ttl_purge_enabled);
        assert_eq!(config.ttl_purge_interval_secs, 3600);
        assert!(!config.index_gc_background_enabled);
        assert!(!config.vertex_gc_background_enabled);
        assert!(!config.background_gc_auto_start_enabled);
        assert_eq!(config.background_idle_maintenance_secs, 30);
        assert!(!config.checkpoint_on_close);
        assert_eq!(config.checkpoint_executor_threads, 1);
    }

    #[test]
    fn test_storage_config_validate() {
        let config = StorageConfig::default();
        assert!(config.validate().is_ok());

        let invalid_config = StorageConfig {
            compression_level: 10,
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());
    }

    #[test]
    fn test_query_resource_config_default() {
        let config = QueryResourceConfig::default();
        assert_eq!(config.max_concurrent_queries, 100);
        assert_eq!(config.max_vertex_scan, 1_000_000);
        assert_eq!(config.max_edge_scan, 10_000_000);
        assert!(!config.has_memory_limit());
        assert!(!config.has_timeout());
        assert_eq!(config.max_result_size, 0);
        assert!(!config.has_result_size_limit());
    }

    #[test]
    fn test_query_resource_config_validate() {
        let config = QueryResourceConfig::default();
        assert!(config.validate().is_ok());

        let invalid_config = QueryResourceConfig {
            max_concurrent_queries: 0,
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());
    }

    #[test]
    fn test_result_size_limit_bounds() {
        let unlimited = QueryResourceConfig::default();
        assert!(!unlimited.has_result_size_limit());

        let single = QueryResourceConfig {
            max_result_size: 1,
            ..Default::default()
        };
        assert!(single.has_result_size_limit());

        let bounded = QueryResourceConfig {
            max_result_size: 1000,
            ..Default::default()
        };
        assert!(bounded.has_result_size_limit());
        assert_eq!(bounded.max_result_size, 1000);
    }

    #[test]
    fn test_storage_engine_display() {
        assert_eq!(StorageEngine::PropertyGraph.to_string(), "propertygraph");
        assert_eq!(StorageEngine::RocksDB.to_string(), "rocksdb");
    }

    #[test]
    fn test_compression_algorithm_display() {
        assert_eq!(CompressionAlgorithm::None.to_string(), "none");
        assert_eq!(CompressionAlgorithm::Zstd.to_string(), "zstd");
    }
}
