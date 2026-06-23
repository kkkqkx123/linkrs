//! Node Estimator Module
//!
//! Provide a cost estimation function for different types of plan nodes.

use crate::query::optimizer::cost::estimate::NodeCostEstimate;
use crate::query::optimizer::error::CostError;
use crate::query::planning::plan::PlanNodeEnum;

pub mod control_flow;
pub mod data_processing;
pub mod graph_algorithm;
pub mod graph_traversal;
pub mod join;
pub mod scan;
pub mod sort_limit;

pub use control_flow::ControlFlowEstimator;
pub use data_processing::DataProcessingEstimator;
pub use graph_algorithm::GraphAlgorithmEstimator;
pub use graph_traversal::GraphTraversalEstimator;
pub use join::JoinEstimator;
pub use scan::ScanEstimator;
pub use sort_limit::SortLimitEstimator;

/// Node Estimator trait
///
/// All node estimators need to implement this trait.
pub trait NodeEstimator {
    /// Estimate the cost of the nodes and the number of output rows.
    ///
    /// # Parameters
    /// – **Node**: The planned execution node.
    /// `child_estimates`: The estimated results of the child nodes
    ///
    /// # Return
    /// `(node_cost, output_rows)`: The cost of the node itself and the estimated number of output rows.
    fn estimate(
        &self,
        node: &PlanNodeEnum,
        child_estimates: &[NodeCostEstimate],
    ) -> Result<(f64, u64), CostError>;
}

/// Get the number of input lines of the child node
pub fn get_input_rows(child_estimates: &[NodeCostEstimate], index: usize) -> u64 {
    child_estimates
        .get(index)
        .map(|e| e.output_rows)
        .unwrap_or(1)
}

/// Calculate the cumulative cost of the child nodes.
pub fn sum_child_costs(child_estimates: &[NodeCostEstimate]) -> f64 {
    child_estimates.iter().map(|e| e.total_cost).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_input_rows() {
        let estimates = vec![
            NodeCostEstimate::new(10.0, 5.0, 100),
            NodeCostEstimate::new(20.0, 10.0, 200),
            NodeCostEstimate::new(30.0, 15.0, 300),
        ];

        assert_eq!(get_input_rows(&estimates, 0), 100);
        assert_eq!(get_input_rows(&estimates, 1), 200);
        assert_eq!(get_input_rows(&estimates, 2), 300);
    }

    #[test]
    fn test_get_input_rows_empty() {
        let estimates: Vec<NodeCostEstimate> = vec![];
        assert_eq!(get_input_rows(&estimates, 0), 1);
    }

    #[test]
    fn test_get_input_rows_out_of_bounds() {
        let estimates = vec![NodeCostEstimate::new(10.0, 5.0, 100)];
        assert_eq!(get_input_rows(&estimates, 1), 1);
    }

    #[test]
    fn test_sum_child_costs() {
        let estimates = vec![
            NodeCostEstimate::new(10.0, 5.0, 100),
            NodeCostEstimate::new(20.0, 10.0, 200),
            NodeCostEstimate::new(30.0, 15.0, 300),
        ];

        let sum = sum_child_costs(&estimates);
        assert_eq!(sum, 30.0);
    }

    #[test]
    fn test_sum_child_costs_empty() {
        let estimates: Vec<NodeCostEstimate> = vec![];
        let sum = sum_child_costs(&estimates);
        assert_eq!(sum, 0.0);
    }

    #[test]
    fn test_sum_child_costs_single() {
        let estimates = vec![NodeCostEstimate::new(10.0, 5.0, 100)];
        let sum = sum_child_costs(&estimates);
        assert_eq!(sum, 5.0);
    }

    #[test]
    fn test_node_cost_estimate_creation() {
        let estimate = NodeCostEstimate::new(10.0, 5.0, 100);
        assert_eq!(estimate.node_cost, 10.0);
        assert_eq!(estimate.total_cost, 5.0);
        assert_eq!(estimate.output_rows, 100);
    }

    #[test]
    fn test_node_cost_estimate_with_zero_values() {
        let estimate = NodeCostEstimate::new(0.0, 0.0, 0);
        assert_eq!(estimate.node_cost, 0.0);
        assert_eq!(estimate.total_cost, 0.0);
        assert_eq!(estimate.output_rows, 0);
    }

    #[test]
    fn test_node_cost_estimate_with_large_values() {
        let estimate = NodeCostEstimate::new(1_000_000.0, 500_000.0, 1_000_000);
        assert_eq!(estimate.node_cost, 1_000_000.0);
        assert_eq!(estimate.total_cost, 500_000.0);
        assert_eq!(estimate.output_rows, 1_000_000);
    }
}
