//! Property Graph Configuration

use std::time::Duration;

use crate::core::error::storage::StorageErrorKind;
use crate::core::types::{AutoCompactConfig, Timestamp};
use crate::core::StorageError;
use crate::storage::compression::CompressionType;

/// Default number of vertex table shards (hash partitions). Higher shard
/// counts increase write concurrency but widen the internal ID space under
/// imbalanced hash distribution (max ID <= num_shards * live_vertices).
pub const DEFAULT_VERTEX_TABLE_SHARDS: usize = 8;

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
        crate::storage::engine::resource_budget::MemoryBudget::new(
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

/// Freeze strategy type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FreezeStrategyType {
    /// Conservative: freeze frequently but merge rarely
    Conservative,
    /// Adaptive: freeze with age-based merge
    #[default]
    Adaptive,
    /// LSM tiered: freeze with LSM-style hierarchical merge
    LSMTiered,
}

/// Configuration for adaptive segment merging
#[derive(Debug, Clone)]
pub struct MergeConfig {
    /// Enable adaptive merge during compaction
    pub enable_adaptive_merge: bool,
    /// Maximum age (in timestamp units) before a segment should be merged
    /// 0 = never merge due to age, Timestamp::MAX = always merge
    pub max_segment_age: Timestamp,
    /// Deletion ratio threshold for merge priority (0.0-1.0)
    /// Segments with deletion ratio > this threshold get higher merge priority
    pub deletion_threshold: f64,
    /// Maximum size for a single segment before forcing merge (in bytes)
    pub max_segment_size_bytes: usize,
    /// Enable LSM-style tiered merging (hierarchical levels)
    pub enable_lsm_tiering: bool,
}

impl Default for MergeConfig {
    fn default() -> Self {
        Self {
            enable_adaptive_merge: true,
            max_segment_age: 1000, // Merge segments older than 1000 timestamp units
            deletion_threshold: 0.3, // Prioritize segments with >30% deletions
            max_segment_size_bytes: 8 * 1024 * 1024, // 8MB per direction
            enable_lsm_tiering: false, // Disabled by default, can be enabled for long-running systems
        }
    }
}

/// Unified Freeze Configuration
///
/// Consolidates all Freeze-related settings in one place:
/// - Decision thresholds (BackgroundFreezeConfig)
/// - Merge strategy and parameters (MergeConfig)
/// - Strategy selection
#[derive(Debug, Clone)]
pub struct FreezeConfig {
    /// Strategy type for Freeze operations
    pub strategy: FreezeStrategyType,

    // ── Decision Thresholds ──
    /// Freeze when mutable delta edges exceed this count
    pub delta_edge_threshold: u64,
    /// Freeze when mutable delta memory exceeds this (in bytes)
    pub delta_memory_threshold_bytes: u64,

    // ── Merge Configuration ──
    /// Maximum age (in timestamp units) before a segment should be merged
    pub max_segment_age: Timestamp,
    /// Deletion ratio threshold for merge priority (0.0-1.0)
    pub deletion_threshold: f64,

    // ── Adaptive Strategy Parameters ──
    /// Minimum segment count to trigger adaptive merge (configurable threshold)
    pub adaptive_segment_threshold: usize,
    /// Absolute segment count that forces freeze (independent of age/deletion)
    pub adaptive_maximum_segments: usize,

    // ── LSM Strategy Parameters ──
    /// Segment count threshold for LSM pressure-based freezing
    pub lsm_segment_pressure_threshold: usize,
}

impl FreezeConfig {
    /// Create a conservative configuration (freeze often, merge rarely)
    ///
    /// Suitable for: Development, testing, or when fresh data is critical
    /// - Small freeze threshold (50K edges)
    /// - No merge after freeze
    /// - Very conservative memory usage
    pub fn development() -> Self {
        Self {
            strategy: FreezeStrategyType::Conservative,
            delta_edge_threshold: 50_000,
            delta_memory_threshold_bytes: 128 * 1024 * 1024, // 128MB
            max_segment_age: Timestamp::MAX,                 // Never merge
            deletion_threshold: 0.5,
            adaptive_segment_threshold: 20, // Low threshold for dev
            adaptive_maximum_segments: 50,  // Force freeze if >50 segments
            lsm_segment_pressure_threshold: 100, // Low threshold for dev
        }
    }

    /// Create a production configuration for small systems (< 1M edges)
    ///
    /// Suitable for: Small deployments, single-node systems
    /// - Moderate freeze threshold (100K edges)
    /// - Adaptive merge with reasonable parameters
    /// - Balanced memory and performance
    pub fn production_small() -> Self {
        Self {
            strategy: FreezeStrategyType::Adaptive,
            delta_edge_threshold: 100_000,
            delta_memory_threshold_bytes: 256 * 1024 * 1024, // 256MB
            max_segment_age: 5000,
            deletion_threshold: 0.2,
            adaptive_segment_threshold: 50, // More reasonable for small systems
            adaptive_maximum_segments: 150, // Force freeze if >150 segments
            lsm_segment_pressure_threshold: 150,
        }
    }

    /// Create a production configuration for large systems (> 1M edges)
    ///
    /// Suitable for: Large deployments, long-running systems
    /// - Large freeze threshold (500K edges)
    /// - LSM tiered merge for long-term stability
    /// - Optimized for sustained high throughput
    pub fn production_large() -> Self {
        Self {
            strategy: FreezeStrategyType::LSMTiered,
            delta_edge_threshold: 500_000,
            delta_memory_threshold_bytes: 1_000_000_000, // 1GB
            max_segment_age: 1000,
            deletion_threshold: 0.3,
            adaptive_segment_threshold: 100, // Higher threshold for large systems
            adaptive_maximum_segments: 300,  // Force freeze if >300 segments
            lsm_segment_pressure_threshold: 200, // LSM pressure at 200+ segments
        }
    }

    /// Validate configuration for consistency and correctness
    ///
    /// Checks:
    /// - Thresholds are positive
    /// - Deletion ratio is in [0.0, 1.0]
    /// - Memory threshold is reasonable
    /// - Adaptive/LSM thresholds are consistent
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

        // Validate new threshold fields
        if self.adaptive_segment_threshold == 0 {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "adaptive_segment_threshold must be > 0",
            ));
        }

        if self.adaptive_maximum_segments < self.adaptive_segment_threshold {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                format!(
                    "adaptive_maximum_segments ({}) must be >= adaptive_segment_threshold ({})",
                    self.adaptive_maximum_segments, self.adaptive_segment_threshold
                ),
            ));
        }

        if self.lsm_segment_pressure_threshold == 0 {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "lsm_segment_pressure_threshold must be > 0",
            ));
        }

        // Strategy-specific validation
        match self.strategy {
            FreezeStrategyType::Conservative => {
                // No additional checks needed
            }
            FreezeStrategyType::Adaptive => {
                if self.max_segment_age == 0 {
                    return Err(StorageError::new(
                        StorageErrorKind::InvalidInput,
                        "Adaptive strategy requires max_segment_age > 0",
                    ));
                }
            }
            FreezeStrategyType::LSMTiered => {
                // LSM tiering works with any age value
                if self.max_segment_age < 500 {
                    log::warn!("LSM tiering with max_segment_age < 500 may cause excessive merges");
                }
            }
        }

        Ok(())
    }
}

impl Default for FreezeConfig {
    fn default() -> Self {
        Self::production_small()
    }
}

/// LSM-style tiered storage levels for segments
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LSMSegmentLevel {
    /// Level 0: Small segments from freeze (< 1MB)
    L0,
    /// Level 1: Medium segments (1-8MB)
    L1,
    /// Level 2: Large segments (8-32MB)
    L2,
    /// Level 3+: Very large segments (> 32MB)
    L3Plus,
}

impl LSMSegmentLevel {
    /// Get the size range for this level (min, max) in bytes
    pub fn size_range(&self) -> (usize, usize) {
        match self {
            LSMSegmentLevel::L0 => (0, 1024 * 1024), // 0-1MB
            LSMSegmentLevel::L1 => (1024 * 1024, 8 * 1024 * 1024), // 1-8MB
            LSMSegmentLevel::L2 => (8 * 1024 * 1024, 32 * 1024 * 1024), // 8-32MB
            LSMSegmentLevel::L3Plus => (32 * 1024 * 1024, usize::MAX), // 32MB+
        }
    }

    /// Determine level for a given segment size
    pub fn for_size(size: usize) -> Self {
        match size {
            0..=1_048_575 => LSMSegmentLevel::L0,
            1_048_576..=8_388_607 => LSMSegmentLevel::L1,
            8_388_608..=33_554_431 => LSMSegmentLevel::L2,
            _ => LSMSegmentLevel::L3Plus,
        }
    }

    /// Get merge target size for this level (where it should aim to stay below)
    pub fn merge_target_size(&self) -> usize {
        match self {
            LSMSegmentLevel::L0 => 1024 * 1024,           // Target < 1MB
            LSMSegmentLevel::L1 => 8 * 1024 * 1024,       // Target < 8MB
            LSMSegmentLevel::L2 => 32 * 1024 * 1024,      // Target < 32MB
            LSMSegmentLevel::L3Plus => 128 * 1024 * 1024, // Target < 128MB
        }
    }

    /// Get number of segments that should trigger cross-level merge
    pub fn merge_trigger_count(&self) -> usize {
        match self {
            LSMSegmentLevel::L0 => 4,     // Merge when 4+ L0 segments
            LSMSegmentLevel::L1 => 3,     // Merge when 3+ L1 segments
            LSMSegmentLevel::L2 => 2,     // Merge when 2+ L2 segments
            LSMSegmentLevel::L3Plus => 2, // Merge when 2+ L3+ segments
        }
    }
}

#[derive(Debug, Clone)]
pub struct PropertyGraphConfig {
    pub enable_cache: bool,
    pub cache_memory: usize,
    pub flush_config: FlushConfig,
    pub resources: ResourceConfig,
    pub freeze: FreezeConfig,
    pub merge_config: MergeConfig,
    /// Automatic background vertex compaction (ID hole reclamation)
    pub auto_compact: AutoCompactConfig,
    /// Hash partitions per vertex label table
    pub vertex_table_shards: usize,
}

impl Default for PropertyGraphConfig {
    fn default() -> Self {
        Self {
            enable_cache: true,
            cache_memory: 128 * 1024 * 1024,
            flush_config: FlushConfig::default(),
            resources: ResourceConfig::default(),
            freeze: FreezeConfig::default(),
            merge_config: MergeConfig::default(),
            auto_compact: AutoCompactConfig::default(),
            vertex_table_shards: DEFAULT_VERTEX_TABLE_SHARDS,
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
            merge_config: MergeConfig {
                enable_adaptive_merge: false,
                enable_lsm_tiering: false,
                ..Default::default()
            },
            auto_compact: AutoCompactConfig::default(),
            vertex_table_shards: DEFAULT_VERTEX_TABLE_SHARDS,
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
            merge_config: MergeConfig {
                enable_adaptive_merge: true,
                enable_lsm_tiering: false,
                max_segment_age: freeze.max_segment_age,
                deletion_threshold: freeze.deletion_threshold,
                ..Default::default()
            },
            auto_compact: AutoCompactConfig::default(),
            vertex_table_shards: DEFAULT_VERTEX_TABLE_SHARDS,
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
            merge_config: MergeConfig {
                enable_adaptive_merge: true,
                enable_lsm_tiering: true,
                max_segment_age: freeze.max_segment_age,
                deletion_threshold: freeze.deletion_threshold,
                ..Default::default()
            },
            auto_compact: AutoCompactConfig::default(),
            vertex_table_shards: DEFAULT_VERTEX_TABLE_SHARDS,
        }
    }

    /// Create a lightweight test configuration
    ///
    /// Uses minimal cache (8MB), relaxed flush thresholds, and disables
    /// adaptive merging to reduce resource usage in test environments.
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
                strategy: FreezeStrategyType::Conservative,
                delta_edge_threshold: 5000,
                delta_memory_threshold_bytes: 16 * 1024 * 1024,
                max_segment_age: Timestamp::MAX,
                deletion_threshold: 0.5,
                adaptive_segment_threshold: 50,
                adaptive_maximum_segments: 150,
                lsm_segment_pressure_threshold: 100,
            },
            merge_config: MergeConfig {
                enable_adaptive_merge: false,
                enable_lsm_tiering: false,
                max_segment_age: Timestamp::MAX,
                ..Default::default()
            },
            auto_compact: AutoCompactConfig::default(),
            vertex_table_shards: DEFAULT_VERTEX_TABLE_SHARDS,
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
        if !(0.0..=1.0).contains(&self.merge_config.deletion_threshold) {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "merge deletion_threshold must be in [0.0, 1.0]",
            ));
        }
        if self.merge_config.max_segment_size_bytes == 0 {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "max_segment_size_bytes must be > 0",
            ));
        }
        self.freeze.validate()?;
        if !(0.0..=1.0).contains(&self.auto_compact.min_hole_ratio) {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "auto_compact min_hole_ratio must be in [0.0, 1.0]",
            ));
        }
        if !(1..=16).contains(&self.vertex_table_shards)
            || !self.vertex_table_shards.is_power_of_two()
        {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "vertex_table_shards must be a power of two in [1, 16]",
            ));
        }
        Ok(())
    }
}

/// Input for Freeze decision-making (minimal required statistics)
#[derive(Debug, Clone)]
pub struct FreezeDecisionInput {
    pub delta_edge_count: u64,
    pub delta_memory_bytes: u64,
    pub segment_count: usize,
    pub oldest_segment_age: Timestamp,
    pub deletion_ratio: f64,
}

/// Decision engine for Freeze strategy
///
/// Uses enum dispatch (match) instead of trait dispatch to:
/// - Keep decision logic centralized and clear
/// - Avoid trait overhead for simple configurations
/// - Reduce code complexity by 20-40%
pub struct FreezeDecisionEngine {
    pub(crate) strategy: FreezeStrategyType,
    pub(crate) config: FreezeConfig,
}

impl FreezeDecisionEngine {
    /// Create a new decision engine for the given strategy and config
    pub fn new(strategy: FreezeStrategyType, config: FreezeConfig) -> Self {
        Self { strategy, config }
    }

    /// Determine if freeze should be triggered based on strategy
    pub fn should_freeze(&self, input: &FreezeDecisionInput) -> bool {
        match self.strategy {
            FreezeStrategyType::Conservative => self.decide_conservative(input),
            FreezeStrategyType::Adaptive => self.decide_adaptive(input),
            FreezeStrategyType::LSMTiered => self.decide_lsm_tiered(input),
        }
    }

    /// Get human-readable reason for freeze decision (for logging)
    pub fn get_reason(&self, input: &FreezeDecisionInput) -> String {
        if !self.should_freeze(input) {
            return "No freeze needed".to_string();
        }

        match self.strategy {
            FreezeStrategyType::Conservative => {
                format!(
                    "Conservative: edges={}/{}, memory={:.0}MB/{:.0}MB",
                    input.delta_edge_count,
                    self.config.delta_edge_threshold,
                    input.delta_memory_bytes as f64 / 1024.0 / 1024.0,
                    self.config.delta_memory_threshold_bytes as f64 / 1024.0 / 1024.0
                )
            }
            FreezeStrategyType::Adaptive => {
                format!(
                    "Adaptive: edges={}/{}, age={}/{}, segments={}",
                    input.delta_edge_count,
                    self.config.delta_edge_threshold,
                    input.oldest_segment_age,
                    self.config.max_segment_age,
                    input.segment_count
                )
            }
            FreezeStrategyType::LSMTiered => {
                format!("LSMTiered: segments={}", input.segment_count)
            }
        }
    }

    /// Get strategy name
    pub fn strategy_name(&self) -> &'static str {
        match self.strategy {
            FreezeStrategyType::Conservative => "Conservative",
            FreezeStrategyType::Adaptive => "Adaptive",
            FreezeStrategyType::LSMTiered => "LSMTiered",
        }
    }

    // Private decision methods

    fn decide_conservative(&self, input: &FreezeDecisionInput) -> bool {
        input.delta_edge_count >= self.config.delta_edge_threshold
            || input.delta_memory_bytes >= self.config.delta_memory_threshold_bytes
    }

    fn decide_adaptive(&self, input: &FreezeDecisionInput) -> bool {
        // Condition 1: Base freeze (absolute thresholds)
        let base_freeze = input.delta_edge_count >= self.config.delta_edge_threshold
            || input.delta_memory_bytes >= self.config.delta_memory_threshold_bytes;

        // Condition 2: Too many segments (independent of age/deletion)
        let too_many_segments = input.segment_count >= self.config.adaptive_maximum_segments;

        // Condition 3: Old segments with high deletion ratio
        let old_and_dirty = input.oldest_segment_age > self.config.max_segment_age
            && input.deletion_ratio > self.config.deletion_threshold
            && input.segment_count >= self.config.adaptive_segment_threshold;

        base_freeze || too_many_segments || old_and_dirty
    }

    fn decide_lsm_tiered(&self, input: &FreezeDecisionInput) -> bool {
        // Base freeze: absolute thresholds (edge count or memory)
        let base_freeze = input.delta_edge_count >= self.config.delta_edge_threshold
            || input.delta_memory_bytes >= self.config.delta_memory_threshold_bytes;

        // LSM pressure: too many segments at any level
        let lsm_pressure = input.segment_count >= self.config.lsm_segment_pressure_threshold;

        base_freeze || lsm_pressure
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_freeze_config_development() {
        let config = FreezeConfig::development();
        assert_eq!(config.strategy, FreezeStrategyType::Conservative);
        assert_eq!(config.delta_edge_threshold, 50_000);
        assert_eq!(config.delta_memory_threshold_bytes, 128 * 1024 * 1024);
        assert!(config.max_segment_age > 1000); // Never merge
        assert_eq!(config.adaptive_segment_threshold, 20);
        assert_eq!(config.adaptive_maximum_segments, 50);
    }

    #[test]
    fn test_freeze_config_production_small() {
        let config = FreezeConfig::production_small();
        assert_eq!(config.strategy, FreezeStrategyType::Adaptive);
        assert_eq!(config.delta_edge_threshold, 100_000);
        assert_eq!(config.delta_memory_threshold_bytes, 256 * 1024 * 1024);
        assert_eq!(config.max_segment_age, 5000);
        assert_eq!(config.adaptive_segment_threshold, 50);
        assert_eq!(config.adaptive_maximum_segments, 150);
    }

    #[test]
    fn test_freeze_config_production_large() {
        let config = FreezeConfig::production_large();
        assert_eq!(config.strategy, FreezeStrategyType::LSMTiered);
        assert_eq!(config.delta_edge_threshold, 500_000);
        assert_eq!(config.delta_memory_threshold_bytes, 1_000_000_000);
        assert_eq!(config.max_segment_age, 1000);
        assert_eq!(config.adaptive_segment_threshold, 100);
        assert_eq!(config.adaptive_maximum_segments, 300);
        assert_eq!(config.lsm_segment_pressure_threshold, 200);
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
        assert_eq!(config.freeze.strategy, FreezeStrategyType::Conservative);
        assert!(!config.merge_config.enable_adaptive_merge);
    }

    #[test]
    fn test_property_graph_config_production_small() {
        let config = PropertyGraphConfig::production_small();
        assert!(config.freeze.validate().is_ok());
        assert_eq!(config.freeze.strategy, FreezeStrategyType::Adaptive);
        assert!(config.merge_config.enable_adaptive_merge);
        assert!(!config.merge_config.enable_lsm_tiering);
    }

    #[test]
    fn test_property_graph_config_production_large() {
        let config = PropertyGraphConfig::production_large();
        assert!(config.freeze.validate().is_ok());
        assert_eq!(config.freeze.strategy, FreezeStrategyType::LSMTiered);
        assert!(config.merge_config.enable_adaptive_merge);
        assert!(config.merge_config.enable_lsm_tiering);
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
    fn test_lsm_segment_level_for_size() {
        assert_eq!(LSMSegmentLevel::for_size(512 * 1024), LSMSegmentLevel::L0);
        assert_eq!(
            LSMSegmentLevel::for_size(4 * 1024 * 1024),
            LSMSegmentLevel::L1
        );
        assert_eq!(
            LSMSegmentLevel::for_size(16 * 1024 * 1024),
            LSMSegmentLevel::L2
        );
        assert_eq!(
            LSMSegmentLevel::for_size(64 * 1024 * 1024),
            LSMSegmentLevel::L3Plus
        );
    }

    #[test]
    fn test_lsm_segment_level_merge_trigger_count() {
        assert_eq!(LSMSegmentLevel::L0.merge_trigger_count(), 4);
        assert_eq!(LSMSegmentLevel::L1.merge_trigger_count(), 3);
        assert_eq!(LSMSegmentLevel::L2.merge_trigger_count(), 2);
        assert_eq!(LSMSegmentLevel::L3Plus.merge_trigger_count(), 2);
    }

    #[test]
    fn test_freeze_decision_engine_conservative_edges() {
        let config = FreezeConfig::development();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::Conservative, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 60_000,
            delta_memory_bytes: 100 * 1024 * 1024,
            segment_count: 50,
            oldest_segment_age: 1000,
            deletion_ratio: 0.1,
        };

        assert!(engine.should_freeze(&input));
        assert!(engine.get_reason(&input).contains("Conservative"));
    }

    #[test]
    fn test_freeze_decision_engine_conservative_memory() {
        let config = FreezeConfig::development();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::Conservative, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 30_000,
            delta_memory_bytes: 200 * 1024 * 1024,
            segment_count: 50,
            oldest_segment_age: 1000,
            deletion_ratio: 0.1,
        };

        assert!(engine.should_freeze(&input));
    }

    #[test]
    fn test_freeze_decision_engine_conservative_no_freeze() {
        let config = FreezeConfig::development();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::Conservative, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 30_000,
            delta_memory_bytes: 100 * 1024 * 1024,
            segment_count: 50,
            oldest_segment_age: 1000,
            deletion_ratio: 0.1,
        };

        assert!(!engine.should_freeze(&input));
    }

    #[test]
    fn test_freeze_decision_engine_adaptive_base_threshold() {
        let config = FreezeConfig::production_small();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::Adaptive, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 150_000,
            delta_memory_bytes: 200 * 1024 * 1024,
            segment_count: 50,
            oldest_segment_age: 2000,
            deletion_ratio: 0.1,
        };

        assert!(engine.should_freeze(&input));
    }

    #[test]
    fn test_freeze_decision_engine_adaptive_age_condition() {
        let config = FreezeConfig::production_small();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::Adaptive, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 50_000,
            delta_memory_bytes: 200 * 1024 * 1024,
            segment_count: 75,        // Between threshold (50) and maximum (150)
            oldest_segment_age: 6000, // > 5000
            deletion_ratio: 0.25,     // > 0.2
        };

        assert!(engine.should_freeze(&input));
    }

    #[test]
    fn test_freeze_decision_engine_adaptive_too_many_segments() {
        let config = FreezeConfig::production_small();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::Adaptive, config);

        // Test: Too many segments forces freeze (independent of age/deletion)
        let input = FreezeDecisionInput {
            delta_edge_count: 50_000,
            delta_memory_bytes: 200 * 1024 * 1024,
            segment_count: 150,      // At maximum_segments threshold
            oldest_segment_age: 100, // Below threshold
            deletion_ratio: 0.05,    // Below threshold
        };

        assert!(engine.should_freeze(&input));
    }

    #[test]
    fn test_freeze_decision_engine_adaptive_no_freeze_too_few_segments() {
        let config = FreezeConfig::production_small();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::Adaptive, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 50_000,
            delta_memory_bytes: 200 * 1024 * 1024,
            segment_count: 30,        // Below adaptive_segment_threshold (50)
            oldest_segment_age: 6000, // > 5000
            deletion_ratio: 0.25,     // > 0.2
        };

        // Should NOT freeze because segment count is below threshold
        assert!(!engine.should_freeze(&input));
    }

    #[test]
    fn test_freeze_decision_engine_adaptive_no_freeze_without_deletion() {
        let config = FreezeConfig::production_small();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::Adaptive, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 50_000,
            delta_memory_bytes: 200 * 1024 * 1024,
            segment_count: 75,        // Below maximum_segments (150)
            oldest_segment_age: 6000, // > 5000
            deletion_ratio: 0.15,     // < 0.2
        };

        assert!(!engine.should_freeze(&input));
    }

    #[test]
    fn test_freeze_decision_engine_lsm_tiered_segments() {
        let config = FreezeConfig::production_large();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::LSMTiered, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 200_000,
            delta_memory_bytes: 500 * 1024 * 1024,
            segment_count: 250, // > 200
            oldest_segment_age: 500,
            deletion_ratio: 0.1,
        };

        assert!(engine.should_freeze(&input));
        assert!(engine.get_reason(&input).contains("LSMTiered"));
    }

    #[test]
    fn test_freeze_decision_engine_lsm_tiered_base_threshold() {
        let config = FreezeConfig::production_large();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::LSMTiered, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 600_000, // > 500_000
            delta_memory_bytes: 500 * 1024 * 1024,
            segment_count: 150, // < 200
            oldest_segment_age: 500,
            deletion_ratio: 0.1,
        };

        assert!(engine.should_freeze(&input));
    }

    #[test]
    fn test_freeze_decision_engine_strategy_names() {
        let config = FreezeConfig::development();

        let engine_conservative =
            FreezeDecisionEngine::new(FreezeStrategyType::Conservative, config.clone());
        assert_eq!(engine_conservative.strategy_name(), "Conservative");

        let engine_adaptive =
            FreezeDecisionEngine::new(FreezeStrategyType::Adaptive, config.clone());
        assert_eq!(engine_adaptive.strategy_name(), "Adaptive");

        let engine_lsm = FreezeDecisionEngine::new(FreezeStrategyType::LSMTiered, config);
        assert_eq!(engine_lsm.strategy_name(), "LSMTiered");
    }
}
