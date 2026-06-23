//! Graph Algorithm Node Estimator
//!
//! Provide cost estimates for the graph algorithm nodes:
//! - ShortestPath
//! - AllPaths
//! - MultiShortestPath
//! - BFSShortest

use super::NodeEstimator;
use crate::query::optimizer::cost::estimate::NodeCostEstimate;
use crate::query::optimizer::cost::CostCalculator;
use crate::query::optimizer::error::CostError;
use crate::query::planning::plan::PlanNodeEnum;

/// Graph Algorithm Node Estimator
pub struct GraphAlgorithmEstimator<'a> {
    cost_calculator: &'a CostCalculator,
}

impl<'a> GraphAlgorithmEstimator<'a> {
    /// Create a new graph algorithm estimator
    pub fn new(cost_calculator: &'a CostCalculator) -> Self {
        Self { cost_calculator }
    }
}

impl<'a> NodeEstimator for GraphAlgorithmEstimator<'a> {
    fn estimate(
        &self,
        node: &PlanNodeEnum,
        _child_estimates: &[NodeCostEstimate],
    ) -> Result<(f64, u64), CostError> {
        match node {
            PlanNodeEnum::ShortestPath(n) => {
                let max_depth = n.max_step() as u32;
                let cost = self
                    .cost_calculator
                    .calculate_shortest_path_cost(1, max_depth);
                // The “shortest path” function returns a single path.
                Ok((cost, 1))
            }
            PlanNodeEnum::AllPaths(n) => {
                let max_depth = n.max_hop() as u32;
                let cost = self.cost_calculator.calculate_all_paths_cost(1, max_depth);
                // All paths may return multiple results (estimated).
                let output_rows = 2_u64.pow(max_depth.min(10));
                Ok((cost, output_rows))
            }
            PlanNodeEnum::MultiShortestPath(n) => {
                let max_depth = n.steps() as u32;
                let cost = self
                    .cost_calculator
                    .calculate_multi_shortest_path_cost(2, max_depth);
                // The multi-source shortest path algorithm returns multiple paths.
                let output_rows = 2_u64.pow(max_depth.min(10));
                Ok((cost, output_rows))
            }
            PlanNodeEnum::BFSShortest(n) => {
                let max_depth = n.steps() as u32;
                let cost = self
                    .cost_calculator
                    .calculate_shortest_path_cost(1, max_depth);
                Ok((cost, 1))
            }
            _ => Err(CostError::UnsupportedNodeType(format!(
                "The graph algorithm estimator does not support node types: {:?}",
                std::mem::discriminant(node)
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::cost::config::CostModelConfig;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::planning::plan::core::nodes::traversal::{
        AllPathsNode, BFSShortestNode, MultiShortestPathNode, ShortestPathNode,
    };
    use std::sync::Arc;

    fn create_test_calculator() -> CostCalculator {
        let stats_manager = Arc::new(crate::query::optimizer::stats::StatisticsManager::new());
        let config = CostModelConfig::default();
        CostCalculator::with_config(stats_manager, config)
    }

    fn create_test_start_node() -> PlanNodeEnum {
        PlanNodeEnum::Start(StartNode::new())
    }

    #[test]
    fn test_shortest_path_estimation() {
        let calculator = create_test_calculator();
        let estimator = GraphAlgorithmEstimator::new(&calculator);

        let left = create_test_start_node();
        let right = create_test_start_node();
        let node = PlanNodeEnum::ShortestPath(ShortestPathNode::new(
            left,
            right,
            0,
            vec!["friend".to_string()],
            5,
        ));

        let child_estimates = vec![];
        let result = estimator.estimate(&node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 1);
    }

    #[test]
    fn test_all_paths_estimation() {
        let calculator = create_test_calculator();
        let estimator = GraphAlgorithmEstimator::new(&calculator);

        let left = create_test_start_node();
        let right = create_test_start_node();
        let node = PlanNodeEnum::AllPaths(AllPathsNode::new(
            left,
            right,
            0,
            3,
            vec!["friend".to_string()],
            1,
            3,
            false,
        ));

        let child_estimates = vec![];
        let result = estimator.estimate(&node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert!(output_rows > 1);
    }

    #[test]
    fn test_multi_shortest_path_estimation() {
        let calculator = create_test_calculator();
        let estimator = GraphAlgorithmEstimator::new(&calculator);

        let left = create_test_start_node();
        let right = create_test_start_node();
        let node = PlanNodeEnum::MultiShortestPath(MultiShortestPathNode::new(left, right, 4));

        let child_estimates = vec![];
        let result = estimator.estimate(&node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert!(output_rows > 1);
    }

    #[test]
    fn test_bfs_shortest_estimation() {
        let calculator = create_test_calculator();
        let estimator = GraphAlgorithmEstimator::new(&calculator);

        let left = create_test_start_node();
        let right = create_test_start_node();
        let node = PlanNodeEnum::BFSShortest(BFSShortestNode::new(
            left,
            right,
            0,
            3,
            vec!["friend".to_string()],
            false,
        ));

        let child_estimates = vec![];
        let result = estimator.estimate(&node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 1);
    }

    #[test]
    fn test_unsupported_node_type() {
        let calculator = create_test_calculator();
        let estimator = GraphAlgorithmEstimator::new(&calculator);

        let node = PlanNodeEnum::Start(StartNode::new());
        let child_estimates = vec![];
        let result = estimator.estimate(&node, &child_estimates);

        assert!(result.is_err());
    }

    #[test]
    fn test_shortest_path_different_max_steps() {
        let calculator = create_test_calculator();
        let estimator = GraphAlgorithmEstimator::new(&calculator);

        for max_step in [1, 3, 5, 10] {
            let left = create_test_start_node();
            let right = create_test_start_node();
            let node = PlanNodeEnum::ShortestPath(ShortestPathNode::new(
                left,
                right,
                0,
                vec!["friend".to_string()],
                max_step,
            ));

            let child_estimates = vec![];
            let result = estimator.estimate(&node, &child_estimates);

            assert!(result.is_ok());
            let (cost, _) = result.expect("Estimation should succeed");
            assert!(cost > 0.0);
        }
    }

    #[test]
    fn test_all_paths_output_rows_limit() {
        let calculator = create_test_calculator();
        let estimator = GraphAlgorithmEstimator::new(&calculator);

        let left = create_test_start_node();
        let right = create_test_start_node();
        let node = PlanNodeEnum::AllPaths(AllPathsNode::new(
            left,
            right,
            0,
            15,
            vec!["friend".to_string()],
            1,
            15,
            false,
        ));

        let child_estimates = vec![];
        let result = estimator.estimate(&node, &child_estimates);

        assert!(result.is_ok());
        let (_, output_rows) = result.expect("Estimation should succeed");
        assert!(output_rows <= 1024);
    }
}
