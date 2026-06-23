//! Scan Operation Estimator
//!
//! Provide a cost estimate for the scanning nodes:
//! - ScanVertices
//! - ScanEdges
//! - IndexScan
//! - EdgeIndexScan

use super::NodeEstimator;
use crate::query::optimizer::cost::estimate::NodeCostEstimate;
use crate::query::optimizer::cost::CostCalculator;
use crate::query::optimizer::error::CostError;
use crate::query::planning::plan::core::nodes::access::EdgeIndexScanNode;
use crate::query::planning::plan::core::nodes::access::{IndexScanNode, ScanType};
use crate::query::planning::plan::PlanNodeEnum;

/// Scan Operation Estimator
pub struct ScanEstimator<'a> {
    cost_calculator: &'a CostCalculator,
}

impl<'a> ScanEstimator<'a> {
    /// Create a new scanning estimator.
    pub fn new(cost_calculator: &'a CostCalculator) -> Self {
        Self { cost_calculator }
    }

    /// Estimating the selectivity of index scans
    pub fn estimate_index_scan_selectivity(&self, node: &IndexScanNode) -> f64 {
        if node.scan_limits().is_empty() {
            return 0.1;
        }

        let mut total_selectivity: f64 = 1.0;
        for limit in node.scan_limits() {
            let sel = match limit.scan_type {
                ScanType::Unique => 0.01,
                ScanType::Prefix => 0.05,
                ScanType::Range => 0.1,
                ScanType::Full => 1.0,
            };
            total_selectivity *= sel;
        }
        total_selectivity.min(1.0)
    }

    /// Estimating the selectivity of edge index scans
    pub fn estimate_edge_index_scan_selectivity(&self, node: &EdgeIndexScanNode) -> f64 {
        if node.scan_limits().is_empty() {
            return 0.1;
        }

        let mut total_selectivity: f64 = 1.0;
        for limit in node.scan_limits() {
            let sel = match limit.scan_type {
                ScanType::Unique => 0.01,
                ScanType::Prefix => 0.05,
                ScanType::Range => 0.1,
                ScanType::Full => 1.0,
            };
            total_selectivity *= sel;
        }
        total_selectivity.min(1.0)
    }

    /// Obtain the tag name from the IndexScan node.
    fn get_tag_name_from_index_scan(&self, node: &IndexScanNode) -> String {
        // Try to obtain the tag name using the `tag_id`.
        if let Some(tag_name) = self
            .cost_calculator
            .statistics_manager()
            .get_tag_name_by_id(node.tag_id())
        {
            return tag_name;
        }

        // Rollback: Attempt to infer the tag names from the column names in scan_limits.
        if let Some(limit) = node.scan_limits().first() {
            let column = &limit.column;
            if let Some(dot_pos) = column.find('.') {
                return column[..dot_pos].to_string();
            }
        }

        "default".to_string()
    }

    /// Retrieve the attribute names from the IndexScan node.
    fn get_property_name_from_index_scan(&self, node: &IndexScanNode) -> String {
        if let Some(limit) = node.scan_limits().first() {
            limit.column.clone()
        } else {
            "default".to_string()
        }
    }
}

impl<'a> NodeEstimator for ScanEstimator<'a> {
    fn estimate(
        &self,
        node: &PlanNodeEnum,
        _child_estimates: &[NodeCostEstimate],
    ) -> Result<(f64, u64), CostError> {
        match node {
            PlanNodeEnum::ScanVertices(n) => {
                let tag_name = n.tag().map(|s| s.as_str()).unwrap_or("default");
                let row_count = self
                    .cost_calculator
                    .statistics_manager()
                    .get_vertex_count(tag_name);
                let cost = self.cost_calculator.calculate_scan_vertices_cost(tag_name);
                Ok((cost, row_count.max(1)))
            }
            PlanNodeEnum::ScanEdges(n) => {
                let edge_type = n.edge_type().unwrap_or_else(|| "default".to_string());
                let row_count = self
                    .cost_calculator
                    .statistics_manager()
                    .get_edge_count(&edge_type);
                let cost = self.cost_calculator.calculate_scan_edges_cost(&edge_type);
                Ok((cost, row_count.max(1)))
            }
            PlanNodeEnum::IndexScan(n) => {
                let selectivity = self.estimate_index_scan_selectivity(n);
                let tag_name = self.get_tag_name_from_index_scan(n);
                let property_name = self.get_property_name_from_index_scan(n);
                let table_rows = self
                    .cost_calculator
                    .statistics_manager()
                    .get_vertex_count(&tag_name);
                let output_rows = (selectivity * table_rows as f64).max(1.0) as u64;
                let cost = self.cost_calculator.calculate_index_scan_cost(
                    &tag_name,
                    &property_name,
                    selectivity,
                );
                Ok((cost, output_rows))
            }
            PlanNodeEnum::EdgeIndexScan(n) => {
                let edge_type = n.edge_type();
                let selectivity = self.estimate_edge_index_scan_selectivity(n);
                let edge_count = self
                    .cost_calculator
                    .statistics_manager()
                    .get_edge_count(edge_type);
                let output_rows = (selectivity * edge_count as f64).max(1.0) as u64;
                let cost = self
                    .cost_calculator
                    .calculate_edge_index_scan_cost(edge_type, selectivity);
                Ok((cost, output_rows))
            }
            _ => Err(CostError::UnsupportedNodeType(format!(
                "Scan estimator does not support node type: {:?}",
                std::mem::discriminant(node)
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::cost::config::CostModelConfig;
    use crate::query::optimizer::stats::{EdgeTypeStatistics, TagStatistics};
    use crate::query::planning::plan::core::nodes::access::graph_scan_node::*;
    use crate::query::planning::plan::core::nodes::access::{IndexLimit, ScanType};
    use std::sync::Arc;

    fn create_test_calculator() -> CostCalculator {
        let stats_manager = Arc::new(crate::query::optimizer::stats::StatisticsManager::new());
        let config = CostModelConfig::default();
        CostCalculator::with_config(stats_manager, config)
    }

    fn create_test_calculator_with_stats() -> CostCalculator {
        let stats_manager = Arc::new(crate::query::optimizer::stats::StatisticsManager::new());

        let tag_stats = TagStatistics {
            tag_name: "Person".to_string(),
            vertex_count: 1000,
            avg_out_degree: 5.0,
            avg_in_degree: 5.0,
        };
        stats_manager.update_tag_stats(tag_stats);

        let edge_stats = EdgeTypeStatistics {
            edge_type: "friend".to_string(),
            edge_count: 5000,
            avg_out_degree: 3.0,
            avg_in_degree: 2.0,
            max_out_degree: 10,
            max_in_degree: 8,
            unique_src_vertices: 1000,
            out_degree_std_dev: 2.0,
            in_degree_std_dev: 1.5,
            degree_gini_coefficient: 0.3,
            hot_vertices: Vec::new(),
        };
        stats_manager.update_edge_stats(edge_stats);

        let config = CostModelConfig::default();
        CostCalculator::with_config(stats_manager, config)
    }

    #[test]
    fn test_scan_vertices_estimation() {
        let calculator = create_test_calculator_with_stats();
        let estimator = ScanEstimator::new(&calculator);

        let mut node = ScanVerticesNode::new(1, "test_space");
        node.set_tag("Person");
        let plan_node = PlanNodeEnum::ScanVertices(node);

        let child_estimates = vec![];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimates should be successful");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 1000);
    }

    #[test]
    fn test_scan_edges_estimation() {
        let calculator = create_test_calculator_with_stats();
        let estimator = ScanEstimator::new(&calculator);

        let node = ScanEdgesNode::new(1, "friend");
        let plan_node = PlanNodeEnum::ScanEdges(node);

        let child_estimates = vec![];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimates should be successful");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 5000);
    }

    #[test]
    fn test_index_scan_estimation() {
        let calculator = create_test_calculator_with_stats();
        let estimator = ScanEstimator::new(&calculator);

        let mut node = IndexScanNode::new(
            1,
            1,
            1,
            "test_index".to_string(),
            "test_schema".to_string(),
            ScanType::Unique,
        );
        node.set_scan_limits(vec![IndexLimit::equal("Person.name", "Alice")]);
        let plan_node = PlanNodeEnum::IndexScan(node);

        let child_estimates = vec![];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimates should be successful");
        assert!(cost > 0.0);
        assert!(output_rows >= 1);
    }

    #[test]
    fn test_edge_index_scan_estimation() {
        let calculator = create_test_calculator_with_stats();
        let estimator = ScanEstimator::new(&calculator);

        let mut node = EdgeIndexScanNode::new(1, "friend", "friend_index");
        node.set_scan_limits(vec![IndexLimit::equal("friend.rank", "1")]);
        let plan_node = PlanNodeEnum::EdgeIndexScan(node);

        let child_estimates = vec![];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimates should be successful");
        assert!(cost > 0.0);
        assert!(output_rows >= 1);
    }

    #[test]
    fn test_unsupported_node_type() {
        let calculator = create_test_calculator();
        let estimator = ScanEstimator::new(&calculator);

        let node = PlanNodeEnum::Start(
            crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode::new(),
        );
        let child_estimates = vec![];
        let result = estimator.estimate(&node, &child_estimates);

        assert!(result.is_err());
    }

    #[test]
    fn test_estimate_index_scan_selectivity_empty() {
        let calculator = create_test_calculator();
        let estimator = ScanEstimator::new(&calculator);

        let node = IndexScanNode::new(
            1,
            1,
            1,
            "test_index".to_string(),
            "test_schema".to_string(),
            ScanType::Unique,
        );
        let selectivity = estimator.estimate_index_scan_selectivity(&node);
        assert_eq!(selectivity, 0.1);
    }

    #[test]
    fn test_estimate_index_scan_selectivity_unique() {
        let calculator = create_test_calculator();
        let estimator = ScanEstimator::new(&calculator);

        let mut node = IndexScanNode::new(
            1,
            1,
            1,
            "test_index".to_string(),
            "test_schema".to_string(),
            ScanType::Unique,
        );
        node.set_scan_limits(vec![IndexLimit::equal("Person.name", "Alice")]);
        let selectivity = estimator.estimate_index_scan_selectivity(&node);
        assert_eq!(selectivity, 0.01);
    }

    #[test]
    fn test_estimate_index_scan_selectivity_prefix() {
        let calculator = create_test_calculator();
        let estimator = ScanEstimator::new(&calculator);

        let mut node = IndexScanNode::new(
            1,
            1,
            1,
            "test_index".to_string(),
            "test_schema".to_string(),
            ScanType::Prefix,
        );
        node.set_scan_limits(vec![IndexLimit::prefix("Person.name", "A")]);
        let selectivity = estimator.estimate_index_scan_selectivity(&node);
        assert_eq!(selectivity, 0.05);
    }

    #[test]
    fn test_estimate_index_scan_selectivity_range() {
        let calculator = create_test_calculator();
        let estimator = ScanEstimator::new(&calculator);

        let mut node = IndexScanNode::new(
            1,
            1,
            1,
            "test_index".to_string(),
            "test_schema".to_string(),
            ScanType::Range,
        );
        node.set_scan_limits(vec![IndexLimit::range(
            "Person.age",
            Some("20"),
            Some("30"),
            true,
            true,
        )]);
        let selectivity = estimator.estimate_index_scan_selectivity(&node);
        assert_eq!(selectivity, 0.1);
    }

    #[test]
    fn test_estimate_index_scan_selectivity_multiple() {
        let calculator = create_test_calculator();
        let estimator = ScanEstimator::new(&calculator);

        let mut node = IndexScanNode::new(
            1,
            1,
            1,
            "test_index".to_string(),
            "test_schema".to_string(),
            ScanType::Unique,
        );
        node.set_scan_limits(vec![
            IndexLimit::equal("Person.name", "Alice"),
            IndexLimit::equal("Person.age", "25"),
        ]);
        let selectivity = estimator.estimate_index_scan_selectivity(&node);
        assert_eq!(selectivity, 0.0001);
    }

    #[test]
    fn test_estimate_edge_index_scan_selectivity_empty() {
        let calculator = create_test_calculator();
        let estimator = ScanEstimator::new(&calculator);

        let node = EdgeIndexScanNode::new(1, "friend", "friend_index");
        let selectivity = estimator.estimate_edge_index_scan_selectivity(&node);
        assert_eq!(selectivity, 0.1);
    }

    #[test]
    fn test_estimate_edge_index_scan_selectivity_unique() {
        let calculator = create_test_calculator();
        let estimator = ScanEstimator::new(&calculator);

        let mut node = EdgeIndexScanNode::new(1, "friend", "friend_index");
        node.set_scan_limits(vec![IndexLimit::equal("friend.rank", "1")]);
        let selectivity = estimator.estimate_edge_index_scan_selectivity(&node);
        assert_eq!(selectivity, 0.01);
    }

    #[test]
    fn test_scan_vertices_no_tag() {
        let calculator = create_test_calculator();
        let estimator = ScanEstimator::new(&calculator);

        let node = ScanVerticesNode::new(1, "test_space");
        let plan_node = PlanNodeEnum::ScanVertices(node);

        let child_estimates = vec![];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert_eq!(cost, 0.0);
        assert_eq!(output_rows, 1);
    }

    #[test]
    fn test_scan_edges_no_edge_type() {
        let calculator = create_test_calculator();
        let estimator = ScanEstimator::new(&calculator);

        let node = ScanEdgesNode::new(1, "");
        let plan_node = PlanNodeEnum::ScanEdges(node);

        let child_estimates = vec![];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert_eq!(cost, 0.0);
        assert_eq!(output_rows, 1);
    }

    #[test]
    fn test_get_tag_name_from_index_scan_with_id() {
        let calculator = create_test_calculator_with_stats();
        let estimator = ScanEstimator::new(&calculator);

        let node = IndexScanNode::new(
            1,
            1,
            1,
            "test_index".to_string(),
            "test_schema".to_string(),
            ScanType::Unique,
        );
        let tag_name = estimator.get_tag_name_from_index_scan(&node);
        assert_eq!(tag_name, "default");
    }

    #[test]
    fn test_get_property_name_from_index_scan() {
        let calculator = create_test_calculator();
        let estimator = ScanEstimator::new(&calculator);

        let mut node = IndexScanNode::new(
            1,
            1,
            1,
            "test_index".to_string(),
            "test_schema".to_string(),
            ScanType::Unique,
        );
        node.set_scan_limits(vec![IndexLimit::equal("Person.name", "Alice")]);
        let property_name = estimator.get_property_name_from_index_scan(&node);
        assert_eq!(property_name, "Person.name");
    }

    #[test]
    fn test_get_property_name_from_index_scan_empty() {
        let calculator = create_test_calculator();
        let estimator = ScanEstimator::new(&calculator);

        let node = IndexScanNode::new(
            1,
            1,
            1,
            "test_index".to_string(),
            "test_schema".to_string(),
            ScanType::Unique,
        );
        let property_name = estimator.get_property_name_from_index_scan(&node);
        assert_eq!(property_name, "default");
    }
}
