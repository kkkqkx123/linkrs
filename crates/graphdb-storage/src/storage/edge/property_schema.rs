//! Property Schema and Statistics
//!
//! Contains schema definitions and compaction statistics for property storage.
//! These types are separated from the table implementation for better modularity.

use crate::core::{DataType, StorageError, StorageResult, Value};
use crate::core::types::Timestamp;

/// Sentinel value meaning "no properties"
pub const PROP_OFFSET_NONE: u32 = 0;

/// Convert a property offset to a row index
/// Offset 0 is the sentinel for "no properties", so row index = offset - 1
pub fn prop_offset_to_index(offset: u32) -> Option<usize> {
    if offset == PROP_OFFSET_NONE {
        return None;
    }
    Some((offset - 1) as usize)
}

/// Convert a row index to a property offset
/// Row index 0 corresponds to offset 1 (since offset 0 is the sentinel)
pub fn prop_index_to_offset(index: usize) -> u32 {
    (index + 1) as u32
}

/// Property schema definition
#[derive(Debug, Clone)]
pub struct PropertySchema {
    pub name: String,
    pub prop_id: i32,
    pub data_type: DataType,
    pub nullable: bool,
}

impl PropertySchema {
    pub fn new(name: String, prop_id: i32, data_type: DataType) -> Self {
        Self {
            name,
            prop_id,
            data_type,
            nullable: false,
        }
    }

    pub fn nullable(mut self, nullable: bool) -> Self {
        self.nullable = nullable;
        self
    }
}

/// Property record with version tracking
/// Supports MVCC-based time-travel queries and garbage collection
#[derive(Debug, Clone)]
pub struct PropertyRecord {
    pub data: Vec<u8>,
    pub create_ts: Timestamp,
    pub delete_ts: Option<Timestamp>,
}

impl PropertyRecord {
    pub fn new(data: Vec<u8>, create_ts: Timestamp) -> Self {
        Self {
            data,
            create_ts,
            delete_ts: None,
        }
    }

    /// Check if this record is visible at the given timestamp
    pub fn is_visible_at(&self, query_ts: Timestamp) -> bool {
        if self.create_ts > query_ts {
            return false;
        }
        if let Some(del_ts) = self.delete_ts {
            if query_ts >= del_ts {
                return false;
            }
        }
        true
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
    /// Includes both deleted record data and PropertyRecord metadata
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
        // Trigger if record fragmentation ratio is high
        let record_fragmentation = if self.total_records == 0 {
            0.0
        } else {
            self.tombstone_count as f64 / self.total_records as f64
        };

        if record_fragmentation > fragmentation_threshold {
            return true;
        }

        // Also trigger if reclaimable bytes are significant
        // (e.g., > 20% of what we could save if we compacted)
        // This handles the case where many small records are deleted
        if self.live_records > 0 && self.reclaimable_bytes > 0 {
            // If reclaimable bytes exceed 50% of total table size, compact
            let total_size = self.live_records * std::mem::size_of::<PropertyRecord>() + self.reclaimable_bytes;
            if total_size > 0 && (self.reclaimable_bytes as f64 / total_size as f64) > 0.5 {
                return true;
            }
        }

        false
    }
}
