use crate::optimizer::stats::StatsView;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::{PartitionSource, PartitionSpec, PartitionStrategy, PlanNodeEnum};

use super::super::config::{PartitioningDecision, PartitioningLayoutInfo};
use super::core::{fallback, PartitioningPlanner};
use super::helpers::split_range;

    /// Choose an edge-scan partition layout for a pure edge-table chain.
    ///
    /// The plan must be a linear chain ending in a `ScanEdges` node whose rows
    /// are self-sufficient (edge properties / aggregates only). Chains that
    /// fetch src/dst vertex properties (Expand, GetProp, AppendVertices) are
    /// rejected because partitioning by src-id range cannot provide the
    /// vertex-side join key.
pub(crate) fn decide_edge_scan(
    planner: &PartitioningPlanner,
        root: &PlanNodeEnum,
        statistics: &StatsView,
        range: &std::ops::Range<i64>,
        layout: &PartitioningLayoutInfo,
    ) -> PartitioningDecision {
        let mut chain = Vec::new();
        if !collect_chain(root, &mut chain) {
            return fallback(
                "edge scan plan is not a linear chain ending in a ScanEdges node",
            );
        }
        let PlanNodeEnum::ScanEdges(scan) = chain[chain.len() - 1] else {
            return fallback(
                "edge scan plan must end in a ScanEdges node for partition selection",
            );
        };
        let Some(edge_type) = scan.edge_type() else {
            return fallback("edge scan has no edge type statistics key");
        };
        let rows = statistics.edge_count(&edge_type);
        if rows == 0 {
            return fallback(format!(
                "missing statistics for edge type '{edge_type}'; cannot estimate row count"
            ));
        }
        if rows < planner.config().min_rows_per_partition.saturating_mul(2) {
            return fallback(format!(
                "estimated edge rows ({rows}) are below the partition threshold"
            ));
        }

        let desired = planner.desired_partition_count(rows);
        if let Some(reason) = planner.cost_gate_reason(rows, rows, desired) {
            return fallback(reason);
        }
        let ranges = split_range(range, desired);
        let source = PartitionSource::EdgeId {
            edge_type: edge_type.to_string(),
        };
        let layout_version =
            planner.layout_signature_with_layout(&source, &PartitionStrategy::Range, layout);
        match PartitionSpec::try_new(ranges, source, Some(layout_version)) {
            Ok(spec) => {
                let description = spec.source().to_string();
                PartitioningDecision {
                    partition_spec: Some(spec),
                    reason: format!(
                        "partitioned edge scan {description} into {desired} src-id ranges from trusted layout"
                    ),
                }
            }
            Err(error) => fallback(format!("invalid configured partition layout: {error}")),
        }
    }

    /// Walk a linear chain from `node` down to its terminal scan. Supports the
    /// same unary operators as the physical partition builder; anything else
    /// (joins, Expand, vertex lookups, set ops) returns `false`.
    fn collect_chain<'a>(node: &'a PlanNodeEnum, chain: &mut Vec<&'a PlanNodeEnum>) -> bool {
        chain.push(node);
        match node {
            PlanNodeEnum::ScanVertices(_) | PlanNodeEnum::ScanEdges(_) => true,
            PlanNodeEnum::Filter(filter) => collect_chain(filter.input(), chain),
            PlanNodeEnum::Project(project) => collect_chain(project.input(), chain),
            PlanNodeEnum::Limit(limit) => collect_chain(limit.input(), chain),
            PlanNodeEnum::Sort(sort) => collect_chain(sort.input(), chain),
            PlanNodeEnum::Aggregate(agg) => collect_chain(agg.input(), chain),
            PlanNodeEnum::TopN(topn) => collect_chain(topn.input(), chain),
            PlanNodeEnum::Dedup(dedup) => collect_chain(dedup.input(), chain),
            PlanNodeEnum::Window(window) => collect_chain(window.input(), chain),
            _ => false,
        }
    }

pub(crate) fn collect_vertex_scans<'a>(
        node: &'a PlanNodeEnum,
        scans: &mut Vec<&'a crate::planning::plan::core::nodes::ScanVerticesNode>,
    ) {
        if let PlanNodeEnum::ScanVertices(scan) = node {
            scans.push(scan);
        }
        for child in node.children() {
            collect_vertex_scans(child, scans);
        }
    }
