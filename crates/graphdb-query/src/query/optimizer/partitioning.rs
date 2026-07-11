//! Conservative physical partition selection for streaming plans.
//!
//! The selector intentionally requires a caller-provided vertex-id domain.
//! Statistics can estimate work, but cannot prove an ID range covers a scan;
//! guessing a full integer range would silently omit non-numeric or sparse
//! identifiers.

use std::ops::Range;

use crate::query::optimizer::StatisticsManager;
use crate::query::planning::plan::{PartitionSpec, PlanNodeEnum};

/// Static configuration for partition selection. The default is disabled so
/// introducing the optimizer cannot change query results without an explicit
/// trusted layout source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitioningConfig {
    pub enabled: bool,
    pub min_rows_per_partition: u64,
    pub max_partitions: usize,
    pub vertex_id_range: Option<Range<u32>>,
}

impl Default for PartitioningConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_rows_per_partition: 100_000,
            max_partitions: 1,
            vertex_id_range: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitioningDecision {
    pub partition_spec: Option<PartitionSpec>,
    pub reason: String,
}

/// Chooses a partition layout only for a single tagged vertex scan. More
/// complex source topologies retain the existing single-tree path until they
/// have an explicit source-domain mapping in the physical planner.
#[derive(Debug, Clone)]
pub struct PartitioningPlanner {
    config: PartitioningConfig,
}

impl PartitioningPlanner {
    pub fn new(config: PartitioningConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &PartitioningConfig {
        &self.config
    }

    pub fn decide(
        &self,
        root: &PlanNodeEnum,
        statistics: &StatisticsManager,
    ) -> PartitioningDecision {
        if !self.config.enabled {
            return Self::fallback("partitioning is disabled");
        }
        if self.config.max_partitions < 2 {
            return Self::fallback("partitioning max_partitions is less than two");
        }
        let Some(range) = self.config.vertex_id_range.clone() else {
            return Self::fallback("no trusted vertex-id range is configured");
        };
        if range.start >= range.end {
            return Self::fallback("configured vertex-id range is empty");
        }

        let mut scans = Vec::new();
        Self::collect_vertex_scans(root, &mut scans);
        if scans.len() != 1 {
            return Self::fallback(
                "automatic partitioning requires exactly one tagged vertex scan",
            );
        }
        let Some(tag) = scans[0].tag() else {
            return Self::fallback("vertex scan has no tag statistics key");
        };
        let rows = statistics.get_vertex_count(tag);
        if rows < self.config.min_rows_per_partition.saturating_mul(2) {
            return Self::fallback(format!(
                "estimated vertex rows ({rows}) are below the partition threshold"
            ));
        }

        let desired = usize::try_from(rows / self.config.min_rows_per_partition)
            .unwrap_or(self.config.max_partitions)
            .clamp(2, self.config.max_partitions);
        let ranges = split_range(range, desired);
        match PartitionSpec::try_new(ranges) {
            Ok(spec) => PartitioningDecision {
                partition_spec: Some(spec),
                reason: format!(
                    "partitioned tagged vertex scan '{}' into {} ranges from trusted layout",
                    tag, desired
                ),
            },
            Err(error) => Self::fallback(format!("invalid configured partition layout: {error}")),
        }
    }

    fn collect_vertex_scans<'a>(
        node: &'a PlanNodeEnum,
        scans: &mut Vec<&'a crate::query::planning::plan::core::nodes::ScanVerticesNode>,
    ) {
        if let PlanNodeEnum::ScanVertices(scan) = node {
            scans.push(scan);
        }
        for child in node.children() {
            Self::collect_vertex_scans(&child, scans);
        }
    }

    fn fallback(reason: impl Into<String>) -> PartitioningDecision {
        PartitioningDecision {
            partition_spec: None,
            reason: reason.into(),
        }
    }
}

fn split_range(range: Range<u32>, partition_count: usize) -> Vec<Range<u32>> {
    let total = range.end - range.start;
    let width = total.div_ceil(partition_count as u32);
    let mut ranges = Vec::with_capacity(partition_count);
    for index in 0..partition_count {
        let start = range.start + (index as u32) * width;
        if start >= range.end {
            break;
        }
        ranges.push(start..(start + width).min(range.end));
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::TagStatistics;
    use crate::query::planning::plan::core::nodes::ScanVerticesNode;

    fn tagged_scan() -> PlanNodeEnum {
        let mut scan = ScanVerticesNode::new(1, "space");
        scan.set_tag("person");
        PlanNodeEnum::ScanVertices(scan)
    }

    #[test]
    fn selects_only_with_trusted_range_and_sufficient_statistics() {
        let stats = StatisticsManager::new();
        let mut tag = TagStatistics::new("person".to_string());
        tag.vertex_count = 10_000;
        stats.update_tag_stats(tag);
        let planner = PartitioningPlanner::new(PartitioningConfig {
            enabled: true,
            min_rows_per_partition: 1_000,
            max_partitions: 4,
            vertex_id_range: Some(0..10_000),
        });

        let decision = planner.decide(&tagged_scan(), &stats);
        assert_eq!(
            decision
                .partition_spec
                .as_ref()
                .map(PartitionSpec::partition_count),
            Some(4)
        );
    }

    #[test]
    fn falls_back_without_a_trusted_range() {
        let stats = StatisticsManager::new();
        let planner = PartitioningPlanner::new(PartitioningConfig {
            enabled: true,
            max_partitions: 4,
            ..PartitioningConfig::default()
        });

        let decision = planner.decide(&tagged_scan(), &stats);
        assert!(decision.partition_spec.is_none());
        assert!(decision.reason.contains("trusted vertex-id range"));
    }
}
