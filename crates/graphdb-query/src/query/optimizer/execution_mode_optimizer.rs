//! Execution Mode Optimizer Module
//!
//! Phase 3 of query optimization: decides whether a query should execute in
//! Streaming or Materialized mode based on cost analysis.
//!
//! This is a critical architectural component that replaces the old decision.rs
//! pattern (which incorrectly placed execution mode selection in the executor layer).
//! Now execution mode selection happens during optimization, allowing for:
//! - Cost-based decisions considering full query characteristics
//! - Consistent execution strategy across complex query plans
//! - Better optimization opportunities (pushing decisions to optimizer phase)

use crate::query::optimizer::error::OptimizeResult;
use crate::query::optimizer::stats::StatisticsManager;
use crate::query::planning::plan::{ExecutionMode, PlanNodeEnum};
use std::sync::Arc;

/// Execution Mode Optimizer - Phase 3 of query optimization
///
/// Responsible for deciding whether a query plan should use streaming or materialized
/// execution based on cost estimation and plan characteristics.
#[derive(Debug, Clone)]
pub struct ExecutionModeOptimizer {
    stats_manager: Arc<StatisticsManager>,
}

impl ExecutionModeOptimizer {
    /// Create a new ExecutionModeOptimizer
    pub fn new(stats_manager: Arc<StatisticsManager>) -> Self {
        Self { stats_manager }
    }

    /// Decide the execution mode for a query plan
    ///
    /// # Returns
    /// A tuple of (ExecutionMode, reason_string)
    pub fn decide_execution_mode(
        &self,
        root: &PlanNodeEnum,
    ) -> OptimizeResult<(ExecutionMode, String)> {
        // Check if plan is streamable (all nodes supported in streaming mode)
        if !self.is_plan_streamable(root) {
            return Ok((
                ExecutionMode::Materialized,
                "Plan contains nodes not supported in streaming mode".to_string(),
            ));
        }

        // Estimate costs for both modes
        let streaming_cost = self.estimate_streaming_cost(root)?;
        let materialized_cost = self.estimate_materialized_cost(root)?;

        // Choose based on cost
        if streaming_cost < materialized_cost {
            Ok((
                ExecutionMode::Streaming,
                format!(
                    "Streaming selected (cost: {:.2} < {:.2})",
                    streaming_cost, materialized_cost
                ),
            ))
        } else {
            Ok((
                ExecutionMode::Materialized,
                format!(
                    "Materialized selected (cost: {:.2} <= {:.2})",
                    materialized_cost, streaming_cost
                ),
            ))
        }
    }

    /// Check if a plan tree is streamable (all nodes support streaming execution)
    fn is_plan_streamable(&self, node: &PlanNodeEnum) -> bool {
        // Check current node
        if !self.is_node_streaming_compatible(node) {
            return false;
        }

        // Recursively check all children
        for child in node.children() {
            if !self.is_plan_streamable(child) {
                return false;
            }
        }

        true
    }

    /// Check if a single node type supports streaming execution
    fn is_node_streaming_compatible(&self, node: &PlanNodeEnum) -> bool {
        // Migrated from decision.rs is_supported_node logic
        // Updated to be more comprehensive
        matches!(
            node,
            // Scan operations
            PlanNodeEnum::ScanVertices(_)
            | PlanNodeEnum::ScanEdges(_)
            // Single-input operations
            | PlanNodeEnum::Filter(_)
            | PlanNodeEnum::Project(_)
            | PlanNodeEnum::Limit(_)
            | PlanNodeEnum::Dedup(_)
            // Aggregations and sorting
            | PlanNodeEnum::Aggregate(_)
            | PlanNodeEnum::Sort(_)
            | PlanNodeEnum::Window(_)
            // Join operations
            | PlanNodeEnum::InnerJoin(_)
            | PlanNodeEnum::LeftJoin(_)
            | PlanNodeEnum::RightJoin(_)
            | PlanNodeEnum::FullOuterJoin(_)
            | PlanNodeEnum::CrossJoin(_)
            | PlanNodeEnum::HashInnerJoin(_)
            | PlanNodeEnum::HashLeftJoin(_)
            // Set operations
            | PlanNodeEnum::Union(_)
            | PlanNodeEnum::Intersect(_)
            | PlanNodeEnum::Minus(_)
            // CTE and materialization
            | PlanNodeEnum::Materialize(_)
            // Other basic operations
            | PlanNodeEnum::Start(_)
        )
    }

    /// Estimate cost of streaming execution
    ///
    /// Streaming advantages:
    /// - Lower memory usage (minimal buffering)
    /// - Lower latency to first result
    ///
    /// Streaming disadvantages:
    /// - Multiple passes for some operations (sort, aggregate)
    /// - Cannot use certain optimizations
    fn estimate_streaming_cost(&self, node: &PlanNodeEnum) -> OptimizeResult<f64> {
        let mut cost = 0.0;

        // Scan cost (row-by-row reads)
        cost += self.estimate_node_cost(node) * 1.0;

        // Add overhead for operations that don't stream well
        cost += self.estimate_streaming_overhead(node)?;

        Ok(cost)
    }

    /// Estimate cost of materialized execution
    ///
    /// Materialized advantages:
    /// - Single pass through data
    /// - Can apply batch optimizations
    /// - Better for sorts/aggregates
    ///
    /// Materialized disadvantages:
    /// - Higher memory usage
    /// - Higher latency to first result
    fn estimate_materialized_cost(&self, node: &PlanNodeEnum) -> OptimizeResult<f64> {
        let mut cost = 0.0;

        // Scan cost (can be optimized via batch)
        cost += self.estimate_node_cost(node) * 0.9;

        // Add memory cost
        let estimated_rows = self.estimate_rows(node);
        let row_size = 128; // Average row size in bytes
        let memory_cost = (estimated_rows as f64 * row_size as f64) * 0.0001;
        cost += memory_cost;

        Ok(cost)
    }

    /// Estimate streaming-specific overhead (operations that don't parallelize well)
    fn estimate_streaming_overhead(&self, node: &PlanNodeEnum) -> OptimizeResult<f64> {
        match node {
            // Sort requires buffering all data in streaming mode
            PlanNodeEnum::Sort(_) => {
                let rows = self.estimate_rows(node);
                Ok(rows as f64 * 0.05) // Penalty for streaming sort
            }
            // Aggregate might need buffering in streaming mode
            PlanNodeEnum::Aggregate(_) => {
                let rows = self.estimate_rows(node);
                Ok(rows as f64 * 0.02) // Smaller penalty than sort
            }
            // Other nodes have minimal overhead
            _ => Ok(0.0),
        }
    }

    /// Estimate base cost for a node
    fn estimate_node_cost(&self, node: &PlanNodeEnum) -> f64 {
        match node {
            PlanNodeEnum::ScanVertices(_) => 10.0,
            PlanNodeEnum::ScanEdges(_) => 12.0,
            PlanNodeEnum::Filter(_) => 1.0,
            PlanNodeEnum::Project(_) => 0.5,
            PlanNodeEnum::Aggregate(_) => 5.0,
            PlanNodeEnum::Sort(_) => 8.0,
            PlanNodeEnum::InnerJoin(_)
            | PlanNodeEnum::LeftJoin(_)
            | PlanNodeEnum::HashInnerJoin(_)
            | PlanNodeEnum::HashLeftJoin(_) => 15.0,
            _ => 1.0,
        }
    }

    /// Estimate number of rows produced by a node
    fn estimate_rows(&self, node: &PlanNodeEnum) -> u64 {
        match node {
            PlanNodeEnum::ScanVertices(scan) => {
                // Try to get stats from statistics manager
                if let Some(tag) = scan.tag() {
                    if let Some(stats) = self.stats_manager.get_tag_stats(tag) {
                        return stats.vertex_count;
                    }
                }
                10000 // Default estimate
            }
            PlanNodeEnum::ScanEdges(_) => 50000, // Edges typically more numerous
            PlanNodeEnum::Filter(_) => {
                // Conservative estimate: 30% selectivity
                let estimated_input_rows = 10000;
                (estimated_input_rows as f64 * 0.3) as u64
            }
            _ => 10000, // Default estimate
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::StartNode;

    #[test]
    fn test_optimizer_creation() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let _optimizer = ExecutionModeOptimizer::new(stats_manager);
    }

    #[test]
    fn test_streaming_compatible_nodes() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let optimizer = ExecutionModeOptimizer::new(stats_manager);

        // Start node should be compatible
        let start = PlanNodeEnum::Start(StartNode::new());
        assert!(optimizer.is_node_streaming_compatible(&start));
    }
}
