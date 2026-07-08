//! Image traversal operation estimator
//!
//! Provide a cost estimate for traversing the nodes in the graph:
//! - Expand
//! - ExpandAll
//! - Traverse
//! - AppendVertices
//! - GetNeighbors
//! - GetVertices
//! - GetEdges

use super::{get_input_rows, NodeEstimator};
use crate::core::types::EdgeDirection;
use crate::query::optimizer::cost::estimate::NodeCostEstimate;
use crate::query::optimizer::cost::CostCalculator;
use crate::query::optimizer::error::CostError;
use crate::query::optimizer::stats::{EdgeTypeStatistics, SkewnessLevel};
use crate::query::planning::plan::PlanNodeEnum;

/// Graph Traversal Operation Estimator
pub struct GraphTraversalEstimator<'a> {
    cost_calculator: &'a CostCalculator,
}

impl<'a> GraphTraversalEstimator<'a> {
    /// Create a new graph traversal estimator.
    pub fn new(cost_calculator: &'a CostCalculator) -> Self {
        Self { cost_calculator }
    }

    /// Obtain the average outdegree for each edge type.
    fn get_avg_out_degree(&self, edge_type: Option<&str>) -> f64 {
        edge_type
            .and_then(|et| self.cost_calculator.statistics_manager().get_edge_stats(et))
            .map(|s| s.avg_out_degree)
            .unwrap_or(2.0)
    }

    /// Obtain the average in-degree for each edge type.
    fn get_avg_in_degree(&self, edge_type: Option<&str>) -> f64 {
        edge_type
            .and_then(|et| self.cost_calculator.statistics_manager().get_edge_stats(et))
            .map(|s| s.avg_in_degree)
            .unwrap_or(2.0)
    }

    /// Obtain the average degree of the edge type (the average of in-degree and out-degree values).
    fn get_avg_degree(&self, edge_type: Option<&str>) -> f64 {
        edge_type
            .and_then(|et| self.cost_calculator.statistics_manager().get_edge_stats(et))
            .map(|s| (s.avg_out_degree + s.avg_in_degree) / 2.0)
            .unwrap_or(2.0)
    }

    /// Obtain statistical information about the type of edges.
    fn get_edge_stats(&self, edge_type: Option<&str>) -> Option<EdgeTypeStatistics> {
        edge_type.and_then(|et| self.cost_calculator.statistics_manager().get_edge_stats(et))
    }

    /// Calculating the extended cost of tilt perception
    fn calculate_skew_aware_expand_cost(
        &self,
        start_rows: u64,
        edge_type: Option<&str>,
        direction: EdgeDirection,
    ) -> f64 {
        let stats = self.get_edge_stats(edge_type);

        match stats {
            Some(s) if s.is_heavily_skewed() => {
                // Calculate the cost based on the inclination and direction.
                let penalty = match s.skewness_level() {
                    SkewnessLevel::Severe => 2.0,
                    SkewnessLevel::Moderate => 1.5,
                    SkewnessLevel::Mild => 1.2,
                    SkewnessLevel::None => 1.0,
                };

                // Select the appropriate inclination angle based on the direction.
                let direction_penalty = match direction {
                    EdgeDirection::Out if s.max_out_degree as f64 > s.avg_out_degree * 5.0 => {
                        penalty * 1.5
                    }
                    EdgeDirection::In if s.max_in_degree as f64 > s.avg_in_degree * 5.0 => {
                        penalty * 1.5
                    }
                    _ => penalty,
                };

                let base_cost = self
                    .cost_calculator
                    .calculate_expand_cost(start_rows, edge_type);
                base_cost * direction_penalty
            }
            _ => self
                .cost_calculator
                .calculate_expand_cost(start_rows, edge_type),
        }
    }

    /// Estimation of the number of output lines for the calculation of inclination perception
    fn estimate_skew_aware_output_rows(
        &self,
        start_rows: u64,
        edge_type: Option<&str>,
        direction: EdgeDirection,
    ) -> u64 {
        let stats = self.get_edge_stats(edge_type);
        let avg_degree = match direction {
            EdgeDirection::Out => self.get_avg_out_degree(edge_type),
            EdgeDirection::In => self.get_avg_in_degree(edge_type),
            EdgeDirection::Both => self.get_avg_degree(edge_type),
        };

        match stats {
            Some(s) if s.is_heavily_skewed() => {
                // For skewed data, it is advisable to use more conservative estimates.
                // Consider the worst-case scenario: all starting nodes are hotspots.
                let conservative_factor = match s.skewness_level() {
                    SkewnessLevel::Severe => 1.5,
                    SkewnessLevel::Moderate => 1.3,
                    SkewnessLevel::Mild => 1.1,
                    SkewnessLevel::None => 1.0,
                };

                (start_rows as f64 * avg_degree * conservative_factor) as u64
            }
            _ => (start_rows as f64 * avg_degree) as u64,
        }
    }
}

impl<'a> NodeEstimator for GraphTraversalEstimator<'a> {
    fn estimate(
        &self,
        node: &PlanNodeEnum,
        child_estimates: &[NodeCostEstimate],
    ) -> Result<(f64, u64), CostError> {
        match node {
            PlanNodeEnum::Expand(n) => {
                let start_rows = get_input_rows(child_estimates, 0);
                let edge_type = n.edge_types().first().map(|s| s.as_str());
                let direction = n.direction();

                // Estimation using tilt sensing
                let output_rows =
                    self.estimate_skew_aware_output_rows(start_rows, edge_type, direction);
                let cost = self.calculate_skew_aware_expand_cost(start_rows, edge_type, direction);

                Ok((cost, output_rows.max(1)))
            }
            PlanNodeEnum::ExpandAll(n) => {
                let start_rows = get_input_rows(child_estimates, 0);
                let edge_type = n.edge_types().first().map(|s| s.as_str());
                // The ExpandAllNode function uses strings to represent directions, and these strings need to be parsed.
                let avg_degree = match n.direction() {
                    "IN" | "in" | "In" => self.get_avg_in_degree(edge_type),
                    "BOTH" | "both" | "Both" => self.get_avg_degree(edge_type),
                    _ => self.get_avg_out_degree(edge_type), // By default, it is displayed outside.
                };
                let output_rows = (start_rows as f64 * avg_degree) as u64;
                let cost = self
                    .cost_calculator
                    .calculate_expand_all_cost(start_rows, edge_type);
                Ok((cost, output_rows.max(1)))
            }
            PlanNodeEnum::Traverse(n) => {
                let start_rows = get_input_rows(child_estimates, 0);
                let edge_type = n.edge_types().first().map(|s| s.as_str());
                let steps = n.max_steps();
                // Select the degree value based on the direction of the traversal.
                let avg_degree = match n.direction() {
                    EdgeDirection::Out => self.get_avg_out_degree(edge_type),
                    EdgeDirection::In => self.get_avg_in_degree(edge_type),
                    EdgeDirection::Both => self.get_avg_degree(edge_type),
                };
                // Estimation of the number of output lines for a multi-step traversal
                let output_rows = (start_rows as f64 * avg_degree.powi(steps as i32)) as u64;
                let cost = self
                    .cost_calculator
                    .calculate_traverse_cost(start_rows, edge_type, steps);
                Ok((cost, output_rows.max(1)))
            }
            PlanNodeEnum::AppendVertices(_) => {
                let input_rows_val = get_input_rows(child_estimates, 0);
                let cost = self
                    .cost_calculator
                    .calculate_append_vertices_cost(input_rows_val);
                // The `AppendVertices` method does not change the number of rows.
                Ok((cost, input_rows_val))
            }
            PlanNodeEnum::GetNeighbors(n) => {
                let start_rows = get_input_rows(child_estimates, 0);
                let edge_type = n.edge_types().first().map(|s| s.as_str());
                // The `GetNeighborsNode` function uses strings to represent directions, and these strings need to be parsed.
                let avg_degree = match n.direction() {
                    "IN" | "in" | "In" => self.get_avg_in_degree(edge_type),
                    "BOTH" | "both" | "Both" => self.get_avg_degree(edge_type),
                    _ => self.get_avg_out_degree(edge_type), // By default, it is displayed outside.
                };
                let output_rows = (start_rows as f64 * avg_degree) as u64;
                let cost = self
                    .cost_calculator
                    .calculate_get_neighbors_cost(start_rows, edge_type);
                Ok((cost, output_rows.max(1)))
            }
            PlanNodeEnum::GetVertices(n) => {
                let vid_count = n.limit().unwrap_or(100) as u64;
                let cost = self.cost_calculator.calculate_get_vertices_cost(vid_count);
                Ok((cost, vid_count))
            }
            PlanNodeEnum::GetEdges(n) => {
                let edge_count = n.limit().unwrap_or(100) as u64;
                let cost = self.cost_calculator.calculate_get_edges_cost(edge_count);
                Ok((cost, edge_count))
            }
            _ => Err(CostError::UnsupportedNodeType(format!(
                "The graph traversal estimator does not support the node type: {:?}",
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
    use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
        MultipleInputNode, SingleInputNode,
    };
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::planning::plan::core::nodes::traversal::traversal_node::*;
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

    fn create_test_start_node() -> PlanNodeEnum {
        PlanNodeEnum::Start(StartNode::new())
    }

    #[test]
    fn test_expand_estimation() {
        let calculator = create_test_calculator_with_stats();
        let estimator = GraphTraversalEstimator::new(&calculator);

        let input = create_test_start_node();
        let mut node = ExpandNode::new(1, vec!["friend".to_string()], EdgeDirection::Out);
        node.add_input(input);
        let plan_node = PlanNodeEnum::Expand(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert!(output_rows >= 1);
    }

    #[test]
    fn test_expand_all_estimation() {
        let calculator = create_test_calculator_with_stats();
        let estimator = GraphTraversalEstimator::new(&calculator);

        let input = create_test_start_node();
        let mut node = ExpandAllNode::new(1, vec!["friend".to_string()], "OUT");
        node.add_input(input);
        let plan_node = PlanNodeEnum::ExpandAll(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert!(output_rows >= 1);
    }

    #[test]
    fn test_traverse_estimation() {
        let calculator = create_test_calculator_with_stats();
        let estimator = GraphTraversalEstimator::new(&calculator);

        let input = create_test_start_node();
        let mut node = TraverseNode::new(1, "vid", 1, 3);
        node.set_input(input);
        let plan_node = PlanNodeEnum::Traverse(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert!(output_rows >= 1);
    }

    #[test]
    fn test_append_vertices_estimation() {
        let calculator = create_test_calculator();
        let estimator = GraphTraversalEstimator::new(&calculator);

        let input = create_test_start_node();
        let mut node = AppendVerticesNode::new(1, "Person");
        node.add_input(input);
        let plan_node = PlanNodeEnum::AppendVertices(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }

    #[test]
    fn test_get_neighbors_estimation() {
        let calculator = create_test_calculator_with_stats();
        let estimator = GraphTraversalEstimator::new(&calculator);

        let input = create_test_start_node();
        let mut node = GetNeighborsNode::new(1, "vid");
        node.add_input(input);
        let plan_node = PlanNodeEnum::GetNeighbors(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert!(output_rows >= 1);
    }

    #[test]
    fn test_get_vertices_estimation() {
        let calculator = create_test_calculator();
        let estimator = GraphTraversalEstimator::new(&calculator);

        let mut node = GetVerticesNode::new(1, "default", "vid");
        node.set_limit(50);
        let plan_node = PlanNodeEnum::GetVertices(node);

        let child_estimates = vec![];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 50);
    }

    #[test]
    fn test_get_edges_estimation() {
        let calculator = create_test_calculator();
        let estimator = GraphTraversalEstimator::new(&calculator);

        let mut node = GetEdgesNode::new(1, "src", "edge", "rank", "dst");
        node.set_limit(100);
        let plan_node = PlanNodeEnum::GetEdges(node);

        let child_estimates = vec![];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }

    #[test]
    fn test_unsupported_node_type() {
        let calculator = create_test_calculator();
        let estimator = GraphTraversalEstimator::new(&calculator);

        let node = PlanNodeEnum::Start(StartNode::new());
        let child_estimates = vec![];
        let result = estimator.estimate(&node, &child_estimates);

        assert!(result.is_err());
    }

    #[test]
    fn test_expand_different_directions() {
        let calculator = create_test_calculator_with_stats();
        let estimator = GraphTraversalEstimator::new(&calculator);

        let input = create_test_start_node();

        for direction in [EdgeDirection::Out, EdgeDirection::In, EdgeDirection::Both] {
            let mut node = ExpandNode::new(1, vec!["friend".to_string()], direction);
            node.add_input(input.clone());
            let plan_node = PlanNodeEnum::Expand(node);

            let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
            let result = estimator.estimate(&plan_node, &child_estimates);

            assert!(result.is_ok());
            let (cost, output_rows) = result.expect("Estimation should succeed");
            assert!(cost > 0.0);
            assert!(output_rows >= 1);
        }
    }

    #[test]
    fn test_traverse_different_steps() {
        let calculator = create_test_calculator_with_stats();
        let estimator = GraphTraversalEstimator::new(&calculator);

        let input = create_test_start_node();

        for (min_steps, max_steps) in [(1, 2), (1, 3), (2, 5)] {
            let mut node = TraverseNode::new(1, "vid", min_steps, max_steps);
            node.set_input(input.clone());
            let plan_node = PlanNodeEnum::Traverse(node);

            let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
            let result = estimator.estimate(&plan_node, &child_estimates);

            assert!(result.is_ok());
            let (cost, output_rows) = result.expect("Estimation should succeed");
            assert!(cost > 0.0);
            assert!(output_rows >= 1);
        }
    }

    #[test]
    fn test_expand_all_direction_parsing() {
        let calculator = create_test_calculator_with_stats();
        let estimator = GraphTraversalEstimator::new(&calculator);

        let input = create_test_start_node();

        for direction in ["OUT", "IN", "BOTH", "out", "in", "both"] {
            let mut node = ExpandAllNode::new(1, vec!["friend".to_string()], direction);
            node.add_input(input.clone());
            let plan_node = PlanNodeEnum::ExpandAll(node);

            let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
            let result = estimator.estimate(&plan_node, &child_estimates);

            assert!(result.is_ok());
            let (cost, output_rows) = result.expect("estimate should succeed");
            assert!(cost > 0.0);
            assert!(output_rows >= 1);
        }
    }

    #[test]
    fn test_get_vertices_no_limit() {
        let calculator = create_test_calculator();
        let estimator = GraphTraversalEstimator::new(&calculator);

        let node = GetVerticesNode::new(1, "default", "vid");
        let plan_node = PlanNodeEnum::GetVertices(node);

        let child_estimates = vec![];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }

    #[test]
    fn test_get_edges_no_limit() {
        let calculator = create_test_calculator();
        let estimator = GraphTraversalEstimator::new(&calculator);

        let node = GetEdgesNode::new(1, "src", "edge", "rank", "dst");
        let plan_node = PlanNodeEnum::GetEdges(node);

        let child_estimates = vec![];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }
}
