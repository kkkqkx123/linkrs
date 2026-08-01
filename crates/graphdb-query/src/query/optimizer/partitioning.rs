//! Conservative physical partition selection for streaming plans.
//!
//! The selector intentionally requires a caller-provided vertex-id domain.
//! Statistics can estimate work, but cannot prove an ID range covers a scan;
//! guessing a full integer range would silently omit non-numeric or sparse
//! identifiers.

use crate::query::optimizer::stats::StatsView;
use crate::query::planning::plan::{PartitionSource, PartitionSpec, PlanNodeEnum};

/// Static configuration for partition selection. The default is disabled so
/// introducing the optimizer cannot change query results without an explicit
/// trusted layout source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitioningConfig {
    pub enabled: bool,
    pub min_rows_per_partition: u64,
    pub max_partitions: usize,
    /// Trusted vertex ID range.  Ranges use `i64` to match the real vertex
    /// ID type and avoid silent truncation of values >= 2^32.
    pub vertex_id_range: Option<std::ops::Range<i64>>,
    /// Maximum worker threads for intra-query parallelism (P8).
    /// 1 means fully serial (P7 fallback).
    pub max_workers: usize,
    /// Maximum queued chunks per partition worker for P8 backpressure.
    pub max_buffered_chunks: usize,
}

impl Default for PartitioningConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_rows_per_partition: 100_000,
            max_partitions: 1,
            vertex_id_range: None,
            max_workers: 1,
            max_buffered_chunks: 10,
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

    pub fn decide(&self, root: &PlanNodeEnum, statistics: &StatsView) -> PartitioningDecision {
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

        // Reject plans with unsupported node categories.
        if Self::has_write_operation(root) {
            return Self::fallback("plan contains write operations; partitioning not supported");
        }
        if Self::has_transaction_boundary(root) {
            return Self::fallback(
                "plan crosses a transaction boundary; partitioning not supported",
            );
        }
        if Self::has_graph_traversal(root) {
            return Self::fallback(
                "plan contains recursive graph traversal; partitioning not supported",
            );
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
        let rows = statistics.vertex_count(tag);
        if rows == 0 {
            return Self::fallback(format!(
                "missing statistics for vertex tag '{tag}'; cannot estimate row count"
            ));
        }
        if rows < self.config.min_rows_per_partition.saturating_mul(2) {
            return Self::fallback(format!(
                "estimated vertex rows ({rows}) are below the partition threshold"
            ));
        }

        let desired = usize::try_from(rows / self.config.min_rows_per_partition)
            .unwrap_or(self.config.max_partitions)
            .clamp(2, self.config.max_partitions);
        let ranges = split_range(range, desired);
        match PartitionSpec::try_new(
            ranges,
            PartitionSource::VertexId {
                tag: tag.to_string(),
            },
            // No layout versioning from the partitioning planner yet;
            // the storage layer will provide one in a later phase.
            None,
        ) {
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
            Self::collect_vertex_scans(child, scans);
        }
    }

    /// Returns true when the plan tree contains any write operation node.
    fn has_write_operation(node: &PlanNodeEnum) -> bool {
        matches!(
            node,
            PlanNodeEnum::InsertVertices(_)
                | PlanNodeEnum::InsertEdges(_)
                | PlanNodeEnum::DeleteVertices(_)
                | PlanNodeEnum::DeleteEdges(_)
                | PlanNodeEnum::DeleteTags(_)
                | PlanNodeEnum::DeleteIndex(_)
                | PlanNodeEnum::PipeDeleteVertices(_)
                | PlanNodeEnum::PipeDeleteEdges(_)
                | PlanNodeEnum::Update(_)
                | PlanNodeEnum::UpdateVertices(_)
                | PlanNodeEnum::UpdateEdges(_)
        ) || node.children().iter().any(|c| Self::has_write_operation(c))
    }

    /// Returns true when the plan tree contains a transaction-control node.
    fn has_transaction_boundary(node: &PlanNodeEnum) -> bool {
        matches!(
            node,
            PlanNodeEnum::BeginTransaction(_) | PlanNodeEnum::Commit(_) | PlanNodeEnum::Rollback(_)
        ) || node
            .children()
            .iter()
            .any(|c| Self::has_transaction_boundary(c))
    }

    /// Returns true when the plan tree contains a recursive graph traversal
    /// or path-algorithm node.
    fn has_graph_traversal(node: &PlanNodeEnum) -> bool {
        matches!(
            node,
            PlanNodeEnum::Expand(_)
                | PlanNodeEnum::ExpandAll(_)
                | PlanNodeEnum::Traverse(_)
                | PlanNodeEnum::AppendVertices(_)
                | PlanNodeEnum::BiExpand(_)
                | PlanNodeEnum::BiTraverse(_)
                | PlanNodeEnum::Loop(_)
                | PlanNodeEnum::MultiShortestPath(_)
                | PlanNodeEnum::BFSShortest(_)
                | PlanNodeEnum::AllPaths(_)
                | PlanNodeEnum::ShortestPath(_)
        ) || node.children().iter().any(|c| Self::has_graph_traversal(c))
    }

    fn fallback(reason: impl Into<String>) -> PartitioningDecision {
        PartitioningDecision {
            partition_spec: None,
            reason: reason.into(),
        }
    }
}

fn split_range(range: std::ops::Range<i64>, partition_count: usize) -> Vec<std::ops::Range<i64>> {
    let total = range.end - range.start;
    let width = (total + partition_count as i64 - 1) / partition_count as i64;
    let mut ranges = Vec::with_capacity(partition_count);
    for index in 0..partition_count {
        let start = range.start + (index as i64) * width;
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
    use crate::query::optimizer::stats::StatisticsManager;
    use crate::query::optimizer::stats::StatsView;
    use crate::query::optimizer::TagStatistics;
    use crate::query::planning::plan::core::nodes::ScanVerticesNode;

    const TEST_SPACE: &str = "test";

    fn tagged_scan() -> PlanNodeEnum {
        let mut scan = ScanVerticesNode::new(1, "space");
        scan.set_tag("person");
        PlanNodeEnum::ScanVertices(scan)
    }

    fn view_of(stats: &StatisticsManager) -> StatsView<'_> {
        StatsView::new(stats, Some(TEST_SPACE))
    }

    #[test]
    fn selects_only_with_trusted_range_and_sufficient_statistics() {
        let stats = StatisticsManager::new();
        let mut tag = TagStatistics::new("person".to_string());
        tag.vertex_count = 10_000;
        stats.update_tag_stats(TEST_SPACE, tag);
        let planner = PartitioningPlanner::new(PartitioningConfig {
            enabled: true,
            min_rows_per_partition: 1_000,
            max_partitions: 4,
            vertex_id_range: Some(0i64..10_000),
            max_workers: 1,
            max_buffered_chunks: 10,
        });

        let decision = planner.decide(&tagged_scan(), &view_of(&stats));
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

        let decision = planner.decide(&tagged_scan(), &view_of(&stats));
        assert!(decision.partition_spec.is_none());
        assert!(decision.reason.contains("trusted vertex-id range"));
    }

    fn make_planner() -> PartitioningPlanner {
        PartitioningPlanner::new(PartitioningConfig {
            enabled: true,
            min_rows_per_partition: 1_000,
            max_partitions: 4,
            vertex_id_range: Some(0i64..10_000),
            max_workers: 1,
            max_buffered_chunks: 10,
        })
    }

    fn make_stats() -> StatisticsManager {
        let stats = StatisticsManager::new();
        let mut tag = TagStatistics::new("person".to_string());
        tag.vertex_count = 10_000;
        stats.update_tag_stats(TEST_SPACE, tag);
        stats
    }

    #[test]
    fn falls_back_on_missing_statistics() {
        let stats = StatisticsManager::new(); // no stats populated
        let plan = tagged_scan();
        let decision = make_planner().decide(&plan, &view_of(&stats));
        assert!(decision.partition_spec.is_none());
        assert!(decision.reason.contains("missing statistics"));
    }

    #[test]
    fn falls_back_on_transaction_boundary() {
        use crate::query::planning::plan::core::nodes::control_flow::control_flow_node::BeginTransactionNode;
        let plan = PlanNodeEnum::BeginTransaction(BeginTransactionNode::new(1));
        let stats = make_stats();
        let decision = make_planner().decide(&plan, &view_of(&stats));
        assert!(decision.partition_spec.is_none());
        assert!(decision.reason.contains("transaction boundary"));
    }

    #[test]
    fn falls_back_on_graph_traversal() {
        use crate::query::planning::plan::core::nodes::traversal::traversal_node::AppendVerticesNode;
        let plan = PlanNodeEnum::AppendVertices(AppendVerticesNode::new(1, "person"));
        let stats = make_stats();
        let decision = make_planner().decide(&plan, &view_of(&stats));
        assert!(decision.partition_spec.is_none());
        assert!(decision.reason.contains("graph traversal"));
    }
}
