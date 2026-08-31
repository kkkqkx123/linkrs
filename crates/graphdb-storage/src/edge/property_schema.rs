//! Property Schema and Statistics
//!
//! Contains schema definitions and compaction statistics for property storage.
//! These types are separated from the table implementation for better modularity.

use crate::encoding::EncodingType;
use graphdb_core::DataType;

/// Default version chain capacity for property storage
pub const DEFAULT_VERSION_CHAIN_CAP: usize = 64;

/// Property schema definition
#[derive(Debug, Clone)]
pub struct PropertySchema {
    pub name: String,
    pub prop_id: i32,
    pub data_type: DataType,
    pub nullable: bool,
    pub encoding_type: EncodingType,
}

impl PropertySchema {
    pub fn new(name: String, prop_id: i32, data_type: DataType) -> Self {
        Self {
            name,
            prop_id,
            data_type,
            nullable: false,
            encoding_type: EncodingType::None,
        }
    }

    pub fn nullable(mut self, nullable: bool) -> Self {
        self.nullable = nullable;
        self
    }

    pub fn with_encoding(mut self, encoding_type: EncodingType) -> Self {
        self.encoding_type = encoding_type;
        self
    }
}

/// Statistics about property table fragmentation and compaction.
///
/// Tracks fragmentation metrics to help decide when to perform compaction
/// and measure the effectiveness of compaction operations.
///
/// Fragmentation is measured in two ways:
/// 1. **Record-level**: tombstone_count / total_records
///    - Quick check for excessive deleted records
///    - Not size-aware (100 large records vs 100 tiny records look the same)
///
/// 2. **Byte-level**: reclaimable_bytes
///    - Actual storage waste from deleted records
///    - More accurate indicator of compaction benefit
///
/// Compaction decisions should primarily consider reclaimable_bytes
/// rather than just fragmentation ratio.
#[derive(Debug, Clone, Default)]
pub struct PropertyCompactionStats {
    /// Number of deleted/tombstoned records
    pub tombstone_count: usize,
    /// Total number of records including tombstones
    pub total_records: usize,
    /// Number of live (non-deleted) records
    /// Equal to total_records - tombstone_count
    pub live_records: usize,
    /// Size of the free list (reusable slots)
    pub free_list_size: usize,
    /// Estimated bytes that could be recovered through compaction
    pub reclaimable_bytes: usize,
}

impl PropertyCompactionStats {
    /// Get fragmentation ratio as a decimal (0.0 to 1.0)
    pub fn fragmentation_ratio(&self) -> f64 {
        if self.total_records == 0 {
            0.0
        } else {
            self.tombstone_count as f64 / self.total_records as f64
        }
    }

    /// Get fragmentation percentage (0-100)
    pub fn fragmentation_percentage(&self) -> f64 {
        self.fragmentation_ratio() * 100.0
    }

    /// Check if compaction should be triggered
    ///
    /// Compaction is beneficial when:
    /// - Record-level fragmentation exceeds threshold, OR
    /// - Reclaimable bytes exceed a significant portion of live data
    ///
    /// This combines both metrics for a more robust decision.
    pub fn should_compact(&self, fragmentation_threshold: f64) -> bool {
        let record_fragmentation = if self.total_records == 0 {
            0.0
        } else {
            self.tombstone_count as f64 / self.total_records as f64
        };

        if record_fragmentation > fragmentation_threshold {
            return true;
        }

        if self.live_records > 0 && self.reclaimable_bytes > 0 {
            // Estimate per-record overhead in columnar layout (row metadata: create_ts + delete_ts + free_list slot)
            const ESTIMATED_BYTES_PER_RECORD: usize = 32;
            let total_size =
                self.live_records * ESTIMATED_BYTES_PER_RECORD + self.reclaimable_bytes;
            if total_size > 0 && (self.reclaimable_bytes as f64 / total_size as f64) > 0.5 {
                return true;
            }
        }

        false
    }
}
