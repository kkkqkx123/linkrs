//! Data partitioning abstractions for parallel execution
//!
//! Defines PartitionView and related traits for splitting execution
//! across multiple partitions (e.g., CPU cores).

use std::ops::Range;

use crate::core::error::QueryError;
use crate::planning::plan::PartitionSpec;

/// A view of data partitioned for parallel processing
///
/// Partitions allow executor to process data independently,
/// enabling parallel execution across CPU cores.
#[derive(Debug, Clone)]
pub struct PartitionView {
    /// Total number of partitions
    pub partition_count: usize,
    /// IDs of all partitions
    pub partition_ids: Vec<usize>,
    /// Range of items (e.g., vertex IDs) for each partition
    pub partition_ranges: Vec<Range<i64>>,
}

/// Infallible conversion from a validated `PartitionSpec`.
/// Skips re-validation because `PartitionSpec::try_new()` already guarantees
/// non-empty, ordered, non-overlapping ranges.
impl From<&PartitionSpec> for PartitionView {
    fn from(spec: &PartitionSpec) -> Self {
        let partition_count = spec.partition_count();
        Self {
            partition_count,
            partition_ids: (0..partition_count).collect(),
            partition_ranges: spec.ranges().to_vec(),
        }
    }
}

impl PartitionView {
    /// Create a validated partition view.
    pub fn try_new(
        partition_count: usize,
        partition_ranges: Vec<Range<i64>>,
    ) -> Result<Self, QueryError> {
        if partition_count == 0 || partition_count != partition_ranges.len() {
            return Err(QueryError::execution(
                "Partition count must match a non-empty range list".to_string(),
            ));
        }

        let mut previous_end = None;
        for (index, range) in partition_ranges.iter().enumerate() {
            if range.start >= range.end {
                return Err(QueryError::execution(format!(
                    "Partition range at index {index} must not be empty"
                )));
            }
            if previous_end.is_some_and(|end| range.start < end) {
                return Err(QueryError::execution(format!(
                    "Partition range at index {index} must be ordered and non-overlapping"
                )));
            }
            previous_end = Some(range.end);
        }

        let partition_ids = (0..partition_count).collect();
        Ok(Self {
            partition_count,
            partition_ids,
            partition_ranges,
        })
    }

    /// Create a single partition (no partitioning)
    pub fn single(range: Range<i64>) -> Result<Self, QueryError> {
        Self::try_new(1, vec![range])
    }

    /// Split a range into N partitions
    pub fn from_range(range: Range<i64>, partition_count: usize) -> Result<Self, QueryError> {
        if partition_count == 0 {
            return Err(QueryError::execution(
                "Partition count must be greater than zero".to_string(),
            ));
        }
        if range.start >= range.end {
            return Err(QueryError::execution(
                "Partition source range must not be empty".to_string(),
            ));
        }
        let total = range.end - range.start;
        let per_partition = (total + partition_count as i64 - 1) / partition_count as i64;

        let mut ranges = Vec::new();
        for i in 0..partition_count {
            let start = range.start + (i as i64) * per_partition;
            let end = (start + per_partition).min(range.end);
            if start < end {
                ranges.push(start..end);
            }
        }

        Self::try_new(ranges.len(), ranges)
    }

    /// Get the range for a specific partition
    pub fn get_range(&self, partition_id: usize) -> Option<Range<i64>> {
        if partition_id < self.partition_ranges.len() {
            Some(self.partition_ranges[partition_id].clone())
        } else {
            None
        }
    }

    /// Number of items in a specific partition
    pub fn partition_size(&self, partition_id: usize) -> i64 {
        self.get_range(partition_id)
            .map(|r| r.end - r.start)
            .unwrap_or(0)
    }

    /// Total number of items across all partitions
    pub fn total_size(&self) -> i64 {
        self.partition_ranges.iter().map(|r| r.end - r.start).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_partition() {
        let view = PartitionView::single(0i64..1000).expect("valid single partition");
        assert_eq!(view.partition_count, 1);
        assert_eq!(view.total_size(), 1000);
        assert_eq!(view.get_range(0), Some(0i64..1000));
    }

    #[test]
    fn test_split_partitions() {
        let view = PartitionView::from_range(0i64..1000, 4).expect("valid split");
        assert_eq!(view.partition_count, 4);
        assert_eq!(view.total_size(), 1000);

        // Check that partitions cover the entire range
        let mut prev_end = 0i64;
        for (i, range) in view.partition_ranges.iter().enumerate() {
            assert_eq!(range.start, prev_end, "Partition {} is not contiguous", i);
            prev_end = range.end;
        }
        assert_eq!(prev_end, 1000);
    }

    #[test]
    fn test_uneven_split() {
        let view = PartitionView::from_range(0i64..1001, 4).expect("valid uneven split");
        assert_eq!(view.partition_count, 4);
        assert_eq!(view.total_size(), 1001);
    }

    #[test]
    fn rejects_invalid_partition_layout() {
        let empty = PartitionView::try_new(0, Vec::new()).expect_err("empty layout must fail");
        assert!(empty.to_string().contains("non-empty"));

        let overlapping = PartitionView::try_new(2, vec![0i64..10, 5i64..20])
            .expect_err("overlapping ranges must fail");
        assert!(overlapping.to_string().contains("non-overlapping"));
    }
}
