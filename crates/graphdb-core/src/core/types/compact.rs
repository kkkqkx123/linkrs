//! Compact Operation Types
//!
//! Provides the core trait and types for storage compaction operations.

use super::storage_ids::Timestamp;

/// Compaction strategy: fixed or adaptive reserve ratio
#[derive(Debug, Clone)]
pub enum CompactionStrategy {
    /// Fixed reserve ratio for all compactions
    Fixed(f32),
    /// Adaptive reserve ratio based on table metrics
    Adaptive(AdaptiveCompactionConfig),
}

impl CompactionStrategy {
    /// Compute the reserve ratio for the current state
    ///
    /// For Fixed, returns the static ratio.
    /// For Adaptive, computes based on edge_count and total_capacity.
    pub fn compute_reserve_ratio(&self, edge_count: usize, total_capacity: usize) -> f32 {
        match self {
            CompactionStrategy::Fixed(ratio) => *ratio,
            CompactionStrategy::Adaptive(config) => config.compute_ratio(edge_count, total_capacity),
        }
    }
}

impl Default for CompactionStrategy {
    fn default() -> Self {
        CompactionStrategy::Fixed(0.8)
    }
}

/// Adaptive compaction configuration
///
/// Reduces reserve_ratio as table grows to save memory in long-running systems.
/// Strategy: tables with high fragmentation get more aggressive compaction.
#[derive(Debug, Clone)]
pub struct AdaptiveCompactionConfig {
    /// Threshold: if edge_count > this, use reduced_ratio instead of base_ratio
    pub size_threshold: usize,
    /// Base reserve ratio for small tables
    pub base_ratio: f32,
    /// Reduced ratio for large tables (more aggressive compaction)
    pub reduced_ratio: f32,
}

impl AdaptiveCompactionConfig {
    pub fn new(size_threshold: usize, base_ratio: f32, reduced_ratio: f32) -> Self {
        Self {
            size_threshold,
            base_ratio: base_ratio.clamp(0.0, 1.0),
            reduced_ratio: reduced_ratio.clamp(0.0, 1.0),
        }
    }

    /// Compute reserve ratio based on edge count
    ///
    /// For small tables: uses base_ratio (e.g., 0.8)
    /// For large tables: linearly interpolates toward reduced_ratio (e.g., 0.2)
    /// This reduces memory pressure in long-running systems while preserving
    /// compaction cost efficiency for smaller, volatile tables.
    fn compute_ratio(&self, edge_count: usize, _total_capacity: usize) -> f32 {
        if edge_count <= self.size_threshold {
            self.base_ratio
        } else {
            self.reduced_ratio
        }
    }
}

impl Default for AdaptiveCompactionConfig {
    fn default() -> Self {
        Self::new(
            100_000, // Use reduced ratio when table exceeds 100k edges
            0.8,     // Base: 80% reserve for small tables (less frequent compaction)
            0.2,     // Reduced: 20% reserve for large tables (more aggressive compaction)
        )
    }
}

/// Configuration for compact operations
#[derive(Debug, Clone)]
pub struct CompactConfig {
    pub enable_structure_compaction: bool,
    pub strategy: CompactionStrategy,
    pub segment_merge_enabled: bool,
    /// Merge segments within this timestamp range
    pub segment_merge_threshold: Timestamp,
    /// Maximum total size (bytes) before triggering segment merge
    /// Default 8MB if segments exceed this, they'll be merged to reduce fragmentation
    pub segment_merge_size_threshold: usize,
    /// Enable adaptive merge strategy based on tombstone pressure
    pub adaptive_merge_enabled: bool,
    /// Tombstone memory threshold (bytes) to trigger aggressive merge
    /// Default 50MB: when tombstone memory exceeds this, use 50% of normal size threshold
    pub tombstone_memory_threshold: usize,
}

impl CompactConfig {
    pub fn new(enable_structure_compaction: bool, strategy: CompactionStrategy) -> Self {
        Self {
            enable_structure_compaction,
            strategy,
            segment_merge_enabled: false,
            segment_merge_threshold: 1000,         // Default: merge segments within 1000 timestamp units
            segment_merge_size_threshold: 8388608, // Default: 8MB
            adaptive_merge_enabled: false,
            tombstone_memory_threshold: 52428800,  // Default: 50MB
        }
    }

    /// Create with fixed reserve ratio (convenience method)
    pub fn with_fixed_ratio(enable_structure_compaction: bool, reserve_ratio: f32) -> Self {
        Self::new(
            enable_structure_compaction,
            CompactionStrategy::Fixed(reserve_ratio.clamp(0.0, 1.0)),
        )
    }

    /// Create with adaptive reserve ratio
    pub fn with_adaptive(enable_structure_compaction: bool) -> Self {
        Self::new(
            enable_structure_compaction,
            CompactionStrategy::Adaptive(AdaptiveCompactionConfig::default()),
        )
    }

    /// Enable segment merging with time and size thresholds
    pub fn enable_segment_merge(mut self, time_threshold: Timestamp) -> Self {
        self.segment_merge_enabled = true;
        self.segment_merge_threshold = time_threshold;
        self
    }

    /// Set size threshold for segment merging (in bytes)
    pub fn with_segment_size_threshold(mut self, size_bytes: usize) -> Self {
        self.segment_merge_size_threshold = size_bytes;
        self
    }

    /// Enable adaptive merge strategy based on tombstone pressure
    ///
    /// When tombstone memory exceeds the threshold, merge size is reduced to 50%
    /// of the normal size threshold, triggering more frequent merges to clean up
    /// tombstones faster.
    pub fn enable_adaptive_merge(mut self, tombstone_memory_threshold: usize) -> Self {
        self.adaptive_merge_enabled = true;
        self.tombstone_memory_threshold = tombstone_memory_threshold;
        self
    }

    /// Compute effective merge size threshold based on tombstone pressure
    ///
    /// If adaptive merge is enabled and tombstone memory exceeds threshold,
    /// returns 50% of the normal size threshold for more aggressive merging.
    pub fn compute_merge_size_threshold(&self, current_tombstone_memory: usize) -> usize {
        if self.adaptive_merge_enabled && current_tombstone_memory > self.tombstone_memory_threshold {
            self.segment_merge_size_threshold / 2
        } else {
            self.segment_merge_size_threshold
        }
    }

    /// Get the computed reserve ratio for current table state
    pub fn compute_reserve_ratio(&self, edge_count: usize, total_capacity: usize) -> f32 {
        self.strategy.compute_reserve_ratio(edge_count, total_capacity)
    }
}

impl Default for CompactConfig {
    fn default() -> Self {
        Self::new(true, CompactionStrategy::Fixed(0.8))
    }
}

/// Statistics about storage compaction
#[derive(Debug, Clone)]
pub struct CompactStats {
    pub total_size: usize,
    pub used_size: usize,
    pub fragmentation_ratio: f32,
}

impl CompactStats {
    pub fn new(total_size: usize, used_size: usize) -> Self {
        let fragmentation_ratio = if total_size > 0 {
            1.0 - (used_size as f32 / total_size as f32)
        } else {
            0.0
        };
        Self {
            total_size,
            used_size,
            fragmentation_ratio,
        }
    }
}

/// Compact transaction result type
pub type CompactResult<T> = Result<T, CompactError>;

/// Compact transaction error
#[derive(Debug, Clone, thiserror::Error)]
pub enum CompactError {
    #[error("Compact operation failed: {0}")]
    CompactFailed(String),

    #[error("Storage error: {0}")]
    StorageError(String),
}

/// Trait for targets that can be compacted
/// This abstracts the storage-specific implementation details from the transaction layer
pub trait CompactTarget: Send + Sync {
    fn compact(&self, config: &CompactConfig, ts: Timestamp) -> CompactResult<()>;
    fn get_compact_stats(&self) -> CompactStats;
}
