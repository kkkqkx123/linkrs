//! Data partitioning abstractions for parallel execution
//!
//! Defines PartitionView and related traits for splitting execution
//! across multiple partitions (e.g., CPU cores).

use std::ops::Range;

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
    pub partition_ranges: Vec<Range<u32>>,
}

impl PartitionView {
    /// Create a new partition view
    pub fn new(partition_count: usize, partition_ranges: Vec<Range<u32>>) -> Self {
        assert_eq!(
            partition_count,
            partition_ranges.len(),
            "Partition count must match ranges"
        );

        let partition_ids = (0..partition_count).collect();
        Self {
            partition_count,
            partition_ids,
            partition_ranges,
        }
    }

    /// Create a single partition (no partitioning)
    pub fn single(range: Range<u32>) -> Self {
        Self::new(1, vec![range])
    }

    /// Split a range into N partitions
    pub fn from_range(range: Range<u32>, partition_count: usize) -> Self {
        let total = range.end - range.start;
        let per_partition = (total + partition_count as u32 - 1) / partition_count as u32;

        let mut ranges = Vec::new();
        for i in 0..partition_count {
            let start = range.start + (i as u32) * per_partition;
            let end = (start + per_partition).min(range.end);
            if start < end {
                ranges.push(start..end);
            }
        }

        Self::new(ranges.len(), ranges)
    }

    /// Get the range for a specific partition
    pub fn get_range(&self, partition_id: usize) -> Option<Range<u32>> {
        if partition_id < self.partition_ranges.len() {
            Some(self.partition_ranges[partition_id].clone())
        } else {
            None
        }
    }

    /// Number of items in a specific partition
    pub fn partition_size(&self, partition_id: usize) -> u32 {
        self.get_range(partition_id)
            .map(|r| r.end - r.start)
            .unwrap_or(0)
    }

    /// Total number of items across all partitions
    pub fn total_size(&self) -> u32 {
        self.partition_ranges
            .iter()
            .map(|r| r.end - r.start)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_partition() {
        let view = PartitionView::single(0..1000);
        assert_eq!(view.partition_count, 1);
        assert_eq!(view.total_size(), 1000);
        assert_eq!(view.get_range(0), Some(0..1000));
    }

    #[test]
    fn test_split_partitions() {
        let view = PartitionView::from_range(0..1000, 4);
        assert_eq!(view.partition_count, 4);
        assert_eq!(view.total_size(), 1000);

        // Check that partitions cover the entire range
        let mut prev_end = 0;
        for (i, range) in view.partition_ranges.iter().enumerate() {
            assert_eq!(range.start, prev_end, "Partition {} is not contiguous", i);
            prev_end = range.end;
        }
        assert_eq!(prev_end, 1000);
    }

    #[test]
    fn test_uneven_split() {
        let view = PartitionView::from_range(0..1001, 4);
        assert_eq!(view.partition_count, 4);
        assert_eq!(view.total_size(), 1001);
    }
}
