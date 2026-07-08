//! Cost Assignment Module
//!
//! Calculate the cost for all nodes in the plan (used only for optimization decisions; the cost is not stored in the nodes).
//!
//! ## Usage Examples
//!
//! ```rust
//! use graphdb::query::optimizer::cost::CostAssigner;
//! use graphdb::query::optimizer::stats::StatisticsManager;
//! use graphdb::query::planner::plan::ExecutionPlan;
//! use std::sync::Arc;
//!
//! let stats_manager = Arc::new(StatisticsManager::new());
//! let assigner = CostAssigner::new(stats_manager);
//!
// Calculate the cost of the execution plan (only for optimization decisions)
//! // let total_cost = assigner.assign_costs(&mut plan)?;
//! ```
//!
//! ## Architecture Description
//!
//! The cost calculation is completely isolated in the optimizer layer and is no longer stored in the PlanNode.
//! The cost is only used for optimizing decisions (such as the selection of indexes, the choice of join algorithms, etc.).
//! Cost information is not required during the execution phase.

use std::sync::Arc;

use crate::query::optimizer::error::CostResult;
use crate::query::optimizer::stats::StatisticsManager;
use crate::query::planning::plan::{ExecutionPlan, PlanNodeEnum};

use super::{
    child_accessor::ChildAccessor,
    estimate::NodeCostEstimate,
    node_estimators::{
        ControlFlowEstimator, DataProcessingEstimator, GraphAlgorithmEstimator,
        GraphTraversalEstimator, JoinEstimator, NodeEstimator, ScanEstimator, SortLimitEstimator,
    },
    CostCalculator, CostModelConfig, SelectivityEstimator,
};

/// Cost Assignment Operator
///
/// Calculate and set the costs for all nodes in the plan.
#[derive(Debug, Clone)]
pub struct CostAssigner {
    cost_calculator: CostCalculator,
    selectivity_estimator: SelectivityEstimator,
    config: CostModelConfig,
}

impl CostAssigner {
    /// Create a new cost allocator (using the default configuration).
    pub fn new(stats_manager: Arc<StatisticsManager>) -> Self {
        Self {
            cost_calculator: CostCalculator::new(stats_manager.clone()),
            selectivity_estimator: SelectivityEstimator::new(stats_manager),
            config: CostModelConfig::default(),
        }
    }

    /// Create a new cost assigner (using the specified configuration).
    pub fn with_config(stats_manager: Arc<StatisticsManager>, config: CostModelConfig) -> Self {
        Self {
            cost_calculator: CostCalculator::with_config(stats_manager.clone(), config),
            selectivity_estimator: SelectivityEstimator::new(stats_manager),
            config,
        }
    }

    /// Obtain the Cost Calculator
    pub fn cost_calculator(&self) -> &CostCalculator {
        &self.cost_calculator
    }

    /// Obtaining a selective estimator
    pub fn selectivity_estimator(&self) -> &SelectivityEstimator {
        &self.selectivity_estimator
    }

    /// Obtain the configuration.
    pub fn config(&self) -> &CostModelConfig {
        &self.config
    }

    /// Assign a cost to the entire execution plan.
    ///
    /// This will recursively traverse the planning tree, calculating and setting the cost for each node.
    pub fn assign_costs(&self, plan: &mut ExecutionPlan) -> CostResult<f64> {
        match plan.root_mut() {
            Some(root) => {
                let estimate = self.assign_node_costs_recursive(root)?;
                Ok(estimate.total_cost)
            }
            None => Ok(0.0),
        }
    }

    /// Assign a cost to the entire execution plan and return a detailed estimate of the results.
    ///
    /// The cost and number of rows estimates for returning to the root node
    pub fn assign_costs_with_estimate(
        &self,
        plan: &mut ExecutionPlan,
    ) -> CostResult<NodeCostEstimate> {
        match plan.root_mut() {
            Some(root) => self.assign_node_costs_recursive(root),
            None => Ok(NodeCostEstimate::new(0.0, 0.0, 0)),
        }
    }

    /// Recursively assign costs to the nodes and their child nodes.
    ///
    /// Use post-order traversal: First, calculate the cost of the child nodes, and then calculate the cost of the current node.
    /// Return the estimation results that include the cost and the number of rows.
    fn assign_node_costs_recursive(&self, node: &mut PlanNodeEnum) -> CostResult<NodeCostEstimate> {
        // 1. First, recursively calculate the cost and the number of rows of the child nodes (using the post-order traversal method).
        let child_estimates = self.calculate_child_estimates(node)?;

        // 2. Calculate the own cost and the number of output rows based on the node type.
        let estimate = self.calculate_node_estimate(node, &child_estimates)?;

        Ok(estimate)
    }

    /// Calculate the cost of child nodes and estimate the number of rows.
    fn calculate_child_estimates(
        &self,
        node: &mut PlanNodeEnum,
    ) -> CostResult<Vec<NodeCostEstimate>> {
        let mut estimates = Vec::new();
        let child_count = node.child_count();

        for i in 0..child_count {
            if let Some(child) = node.get_child_mut(i) {
                let estimate = self.assign_node_costs_recursive(child)?;
                estimates.push(estimate);
            }
        }

        Ok(estimates)
    }

    /// Estimation of the cost of computing nodes and the number of output rows
    fn calculate_node_estimate(
        &self,
        node: &PlanNodeEnum,
        child_estimates: &[NodeCostEstimate],
    ) -> CostResult<NodeCostEstimate> {
        // Calculate the cumulative cost of the child nodes.
        let child_total_cost: f64 = child_estimates.iter().map(|e| e.total_cost).sum();

        // Select the appropriate estimator based on the node type.
        let (node_cost, output_rows) = self.estimate_by_node_type(node, child_estimates)?;

        let total_cost = node_cost + child_total_cost;
        Ok(NodeCostEstimate::new(node_cost, total_cost, output_rows))
    }

    /// Select an estimator based on the node type to perform the estimation.
    fn estimate_by_node_type(
        &self,
        node: &PlanNodeEnum,
        child_estimates: &[NodeCostEstimate],
    ) -> CostResult<(f64, u64)> {
        match node {
            // Scanning operation
            PlanNodeEnum::ScanVertices(_)
            | PlanNodeEnum::ScanEdges(_)
            | PlanNodeEnum::IndexScan(_)
            | PlanNodeEnum::EdgeIndexScan(_) => {
                let estimator = ScanEstimator::new(&self.cost_calculator);
                estimator.estimate(node, child_estimates)
            }

            // Image traversal operations
            PlanNodeEnum::Expand(_)
            | PlanNodeEnum::ExpandAll(_)
            | PlanNodeEnum::Traverse(_)
            | PlanNodeEnum::AppendVertices(_)
            | PlanNodeEnum::GetNeighbors(_)
            | PlanNodeEnum::GetVertices(_)
            | PlanNodeEnum::GetEdges(_) => {
                let estimator = GraphTraversalEstimator::new(&self.cost_calculator);
                estimator.estimate(node, child_estimates)
            }

            // Connection operation
            PlanNodeEnum::HashInnerJoin(_)
            | PlanNodeEnum::HashLeftJoin(_)
            | PlanNodeEnum::InnerJoin(_)
            | PlanNodeEnum::LeftJoin(_)
            | PlanNodeEnum::CrossJoin(_)
            | PlanNodeEnum::FullOuterJoin(_) => {
                let estimator = JoinEstimator::new(&self.cost_calculator);
                estimator.estimate(node, child_estimates)
            }

            // Sorting and filtering operations
            PlanNodeEnum::Sort(_)
            | PlanNodeEnum::Limit(_)
            | PlanNodeEnum::TopN(_)
            | PlanNodeEnum::Aggregate(_)
            | PlanNodeEnum::Dedup(_)
            | PlanNodeEnum::Sample(_) => {
                let estimator = SortLimitEstimator::new(&self.cost_calculator);
                estimator.estimate(node, child_estimates)
            }

            // Set operations: No need for optimization decisions; a conservative estimate is returned.
            PlanNodeEnum::Union(_) | PlanNodeEnum::Minus(_) | PlanNodeEnum::Intersect(_) => {
                let input_rows: u64 = child_estimates.iter().map(|e| e.output_rows).sum();
                Ok((1.0, input_rows.max(1)))
            }

            // Control flow nodes
            PlanNodeEnum::Loop(_)
            | PlanNodeEnum::Select(_)
            | PlanNodeEnum::PassThrough(_)
            | PlanNodeEnum::Argument(_) => {
                let estimator = ControlFlowEstimator::new(&self.cost_calculator, self.config);
                estimator.estimate(node, child_estimates)
            }

            // Graph algorithms
            PlanNodeEnum::ShortestPath(_)
            | PlanNodeEnum::AllPaths(_)
            | PlanNodeEnum::MultiShortestPath(_)
            | PlanNodeEnum::BFSShortest(_) => {
                let estimator = GraphAlgorithmEstimator::new(&self.cost_calculator);
                estimator.estimate(node, child_estimates)
            }

            // Data processing
            PlanNodeEnum::Filter(_)
            | PlanNodeEnum::Project(_)
            | PlanNodeEnum::Unwind(_)
            | PlanNodeEnum::DataCollect(_)
            | PlanNodeEnum::Start(_) => {
                let estimator = DataProcessingEstimator::new(
                    &self.cost_calculator,
                    &self.selectivity_estimator,
                    self.config,
                );
                estimator.estimate(node, child_estimates)
            }

            // Other node types
            _ => {
                // For node types that have not been explicitly processed, return a conservative estimate.
                Ok((1.0, 1))
            }
        }
    }
}

impl Default for CostAssigner {
    fn default() -> Self {
        let stats_manager = Arc::new(StatisticsManager::new());
        let config = CostModelConfig::default();
        Self {
            cost_calculator: CostCalculator::with_config(stats_manager.clone(), config),
            selectivity_estimator: SelectivityEstimator::new(stats_manager),
            config,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cost_assigner_creation() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let assigner = CostAssigner::new(stats_manager);
        assert_eq!(assigner.cost_calculator().config().seq_page_cost, 1.0);
    }

    #[test]
    fn test_cost_assigner_with_config() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let config = CostModelConfig::for_ssd();
        let assigner = CostAssigner::with_config(stats_manager, config);
        assert_eq!(assigner.cost_calculator().config().random_page_cost, 1.1);
    }
}
