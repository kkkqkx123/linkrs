//! CTE (Common Table Expression) Materialization Optimizer Module
//!
//! "Analysis-based optimization strategy for CTE materialization" – Determines whether to materialize a CTE (Common Table Expression) in memory or not.
//!
//! ## Optimization Strategies
//!
//! - Materialize the CTE (Common Table Expression) that will be referenced multiple times to avoid duplicate calculations.
//! - 仅物化确定性的 CTE（不含 rand(), now() 等）
//! - Decisions are made based on the reference count and the size of the result set.
//!
//! ## Applicable Conditions
//!
//! - The number of citations for CTE is greater than 1.
//! - 2. CTE 不包含非确定性函数（如 rand(), now()）
//! - 3. The estimated number of rows for CTE is less than 10,000.
//! 4. The complexity of CTE (Common Table Expression) is less than 80.
//!
//! ## Usage Examples
//!
//! ```rust
//! use graphdb::query::optimizer::strategy::MaterializationOptimizer;
//! use graphdb::query::optimizer::OptimizerEngine;
//!
//! let optimizer = MaterializationOptimizer::new(engine.stats_manager());
//! let decision = optimizer.should_materialize(&cte_node, &plan_root);
//! ```

use crate::query::optimizer::analysis::BatchPlanAnalysis;
use crate::query::optimizer::context::OptimizationContext;
use crate::query::optimizer::cost::StrategyThresholds;
use crate::query::optimizer::cost_based::trait_def::OptimizationStrategy;
use crate::query::optimizer::error::OptimizeResult;
use crate::query::optimizer::stats::StatisticsManager;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::{MaterializeNode, PlanNodeEnum};

/// CTE (Common Table Expression) materialization decision
#[derive(Debug, Clone, PartialEq)]
pub enum MaterializationDecision {
    /// Materialized CTE (Common Table Expression)
    Materialize {
        /// Physical causes
        reason: MaterializeReason,
        /// Number of citations
        reference_count: usize,
        /// Estimating the size of the result set
        estimated_rows: u64,
        /// Estimating the materialization costs
        materialize_cost: f64,
        /// Estimate the cost of redundant calculations
        recompute_cost: f64,
    },
    /// Immaterialization
    DoNotMaterialize {
        /// Reasons for non-materialization
        reason: NoMaterializeReason,
    },
}

/// materialization reasons
#[derive(Debug, Clone, PartialEq)]
pub enum MaterializeReason {
    /// Cited multiple times
    MultipleReferences,
    /// The materialization based on cost analysis is more optimal.
    CostBased,
}

/// Reason for non-materialization
#[derive(Debug, Clone, PartialEq)]
pub enum NoMaterializeReason {
    /// Cited only once
    SingleReference,
    /// Contains non-deterministic functions
    NonDeterministic,
    /// The result set is too large.
    TooLarge,
    /// The expression is too complex.
    TooComplex,
    /// Memory cost is too high
    MemoryCostTooHigh,
}

/// CTE (Common Table Expression) Materialization Optimizer
///
/// Decide whether to materialize a CTE (Common Table Expression) based on batch plan analysis and statistical information.
#[derive(Debug, Clone)]
pub struct MaterializationOptimizer {
    /// Statistical Information Manager
    stats_manager: StatisticsManager,
    /// Threshold for the minimum number of citations
    min_reference_count: usize,
    /// Maximum result set size threshold
    max_result_rows: u64,
    /// Maximum expression complexity threshold
    max_complexity: u32,
    /// Memory cost factor (cost per byte of materialization)
    memory_cost_factor: f64,
    /// Maximum memory cost ratio (memory_cost / recompute_cost)
    max_memory_cost_ratio: f64,
}

impl MaterializationOptimizer {
    /// Create a new optimizer.
    pub fn new(stats_manager: &StatisticsManager) -> Self {
        Self {
            stats_manager: stats_manager.clone(),
            min_reference_count: 2,
            max_result_rows: 10000,
            max_complexity: 80,
            memory_cost_factor: 0.0001, // Default: 0.0001 cost per byte
            max_memory_cost_ratio: 0.5, // Memory cost should not exceed 50% of recompute cost
        }
    }

    /// Create a new optimizer with strategy thresholds from config.
    pub fn with_thresholds(
        stats_manager: &StatisticsManager,
        thresholds: &StrategyThresholds,
    ) -> Self {
        Self {
            stats_manager: stats_manager.clone(),
            min_reference_count: thresholds.min_reference_count,
            max_result_rows: thresholds.max_result_rows,
            max_complexity: 80,
            memory_cost_factor: 0.0001,
            max_memory_cost_ratio: 0.5,
        }
    }

    /// Set memory cost factor
    pub fn with_memory_cost_factor(mut self, factor: f64) -> Self {
        self.memory_cost_factor = factor.max(0.0);
        self
    }

    /// Set maximum memory cost ratio
    pub fn with_max_memory_cost_ratio(mut self, ratio: f64) -> Self {
        self.max_memory_cost_ratio = ratio.max(0.0);
        self
    }

    /// Set a threshold for the minimum number of citations required
    pub fn with_min_reference_count(mut self, count: usize) -> Self {
        self.min_reference_count = count;
        self
    }

    /// Set a threshold for the maximum size of the result set
    pub fn with_max_result_rows(mut self, max_rows: u64) -> Self {
        self.max_result_rows = max_rows;
        self
    }

    /// Set a threshold for the maximum complexity.
    pub fn with_max_complexity(mut self, max_complexity: u32) -> Self {
        self.max_complexity = max_complexity;
        self
    }

    /// Determine whether it is appropriate to materialize the CTE (Common Table Expression).
    ///
    /// # Parameters
    /// `cte_node`: The root node of the CTE (Common Table Expression) sub-plan.
    /// `analysis`: The batch plan analysis result (contains reference count, expression summary, etc.)
    ///
    /// # Return
    /// Materialized Decision Making
    pub fn should_materialize(
        &self,
        cte_node: &PlanNodeEnum,
        analysis: &BatchPlanAnalysis,
    ) -> MaterializationDecision {
        // 1. Check whether CTE is referenced multiple times.
        let ref_info = match analysis
            .reference_count
            .node_reference_map
            .get(&cte_node.id())
        {
            Some(info) => info,
            None => {
                return MaterializationDecision::DoNotMaterialize {
                    reason: NoMaterializeReason::SingleReference,
                }
            }
        };

        if ref_info.reference_count < self.min_reference_count {
            return MaterializationDecision::DoNotMaterialize {
                reason: NoMaterializeReason::SingleReference,
            };
        }

        // 2. Check whether CTE is deterministic.
        if !analysis.expression_summary.is_fully_deterministic {
            return MaterializationDecision::DoNotMaterialize {
                reason: NoMaterializeReason::NonDeterministic,
            };
        }

        // 3. Check the complexity of the expression.
        let complexity = analysis.expression_summary.total_complexity;
        if complexity > self.max_complexity {
            return MaterializationDecision::DoNotMaterialize {
                reason: NoMaterializeReason::TooComplex,
            };
        }

        // 5. Estimating the size of the result set
        let estimated_rows = self.estimate_result_rows(cte_node);
        if estimated_rows > self.max_result_rows {
            return MaterializationDecision::DoNotMaterialize {
                reason: NoMaterializeReason::TooLarge,
            };
        }

        // 6. Comparison of costs
        let recompute_cost = self.estimate_recompute_cost(ref_info.reference_count, estimated_rows);
        let materialize_cost = self.estimate_materialize_cost(estimated_rows, complexity);

        // 7. Check memory cost
        let memory_cost = self.estimate_memory_cost(estimated_rows);
        let total_materialize_cost = materialize_cost + memory_cost;
        let memory_cost_ratio = if recompute_cost > 0.0 {
            memory_cost / recompute_cost
        } else {
            0.0
        };

        // If memory cost is too high relative to recompute cost, don't materialize
        if memory_cost_ratio > self.max_memory_cost_ratio {
            return MaterializationDecision::DoNotMaterialize {
                reason: NoMaterializeReason::MemoryCostTooHigh,
            };
        }

        if total_materialize_cost < recompute_cost {
            MaterializationDecision::Materialize {
                reason: MaterializeReason::CostBased,
                reference_count: ref_info.reference_count,
                estimated_rows,
                materialize_cost: total_materialize_cost,
                recompute_cost,
            }
        } else {
            MaterializationDecision::Materialize {
                reason: MaterializeReason::MultipleReferences,
                reference_count: ref_info.reference_count,
                estimated_rows,
                materialize_cost: total_materialize_cost,
                recompute_cost,
            }
        }
    }

    /// Estimate memory cost for materialization
    fn estimate_memory_cost(&self, estimated_rows: u64) -> f64 {
        // Assume average row size of 64 bytes
        let memory_bytes = estimated_rows * 64;
        memory_bytes as f64 * self.memory_cost_factor
    }

    /// Estimated number of rows in the result set
    fn estimate_result_rows(&self, node: &PlanNodeEnum) -> u64 {
        match node {
            PlanNodeEnum::ScanVertices(n) => {
                if let Some(tag_name) = n.tag() {
                    if let Some(stats) = self.stats_manager.get_tag_stats(tag_name) {
                        stats.vertex_count
                    } else {
                        1000
                    }
                } else {
                    1000
                }
            }
            PlanNodeEnum::Filter(n) => (self.estimate_result_rows(
                crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode::input(
                    n,
                ),
            ) as f64
                * 0.3) as u64,
            PlanNodeEnum::InnerJoin(join_node) => {
                let left_rows = self.estimate_result_rows(join_node.left_input());
                let right_rows = self.estimate_result_rows(join_node.right_input());
                (left_rows as f64 * right_rows as f64 * 0.3) as u64
            }
            PlanNodeEnum::LeftJoin(join_node) => {
                let left_rows = self.estimate_result_rows(join_node.left_input());
                let right_rows = self.estimate_result_rows(join_node.right_input());
                (left_rows as f64 * right_rows as f64 * 0.3) as u64
            }
            PlanNodeEnum::CrossJoin(join_node) => {
                let left_rows = self.estimate_result_rows(join_node.left_input());
                let right_rows = self.estimate_result_rows(join_node.right_input());
                (left_rows as f64 * right_rows as f64 * 0.3) as u64
            }
            PlanNodeEnum::HashInnerJoin(join_node) => {
                let left_rows = self.estimate_result_rows(join_node.left_input());
                let right_rows = self.estimate_result_rows(join_node.right_input());
                (left_rows as f64 * right_rows as f64 * 0.3) as u64
            }
            PlanNodeEnum::HashLeftJoin(join_node) => {
                let left_rows = self.estimate_result_rows(join_node.left_input());
                let right_rows = self.estimate_result_rows(join_node.right_input());
                (left_rows as f64 * right_rows as f64 * 0.3) as u64
            }
            PlanNodeEnum::FullOuterJoin(join_node) => {
                let left_rows = self.estimate_result_rows(join_node.left_input());
                let right_rows = self.estimate_result_rows(join_node.right_input());
                (left_rows as f64 * right_rows as f64 * 0.3) as u64
            }
            PlanNodeEnum::Aggregate(n) => {
                let input_rows = self.estimate_result_rows(crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode::input(n));
                let group_keys = n.group_keys().len();
                if group_keys == 0 {
                    1
                } else {
                    (input_rows as f64 / (group_keys as f64 * 10.0)) as u64
                }
            }
            _ => 1000,
        }
    }

    /// Estimate the cost of double counting
    fn estimate_recompute_cost(&self, reference_count: usize, rows: u64) -> f64 {
        // The calculation must be performed again for each citation.
        (reference_count as f64) * (rows as f64) * 0.1
    }

    /// Estimate physical and chemical costs
    fn estimate_materialize_cost(&self, rows: u64, complexity: u32) -> f64 {
        // Materialization cost = Computational cost + Storage cost
        let compute_cost = (rows as f64) * 0.1;
        let storage_cost = (rows as f64) * 0.05; // Storage costs
        let complexity_overhead = (complexity as f64) * 0.01;

        compute_cost + storage_cost + complexity_overhead
    }

    /// Perform the materialization transformation.
    ///
    /// #Parameters
    /// - `cte_node`: CTE subplan root node
    ///
    /// #Return
    /// Nodes that have the MaterializeNode package installed
    pub fn materialize(
        &self,
        cte_node: PlanNodeEnum,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        let materialize_node = MaterializeNode::new(cte_node)?;
        Ok(PlanNodeEnum::Materialize(materialize_node))
    }
}

impl OptimizationStrategy for MaterializationOptimizer {
    fn apply(&self, node: PlanNodeEnum, ctx: &OptimizationContext) -> OptimizeResult<PlanNodeEnum> {
        // Only optimize MaterializeNode
        if let PlanNodeEnum::Materialize(ref materialize_node) = node {
            // Get batch plan analysis from context, or return node as-is if not available
            let analysis = match ctx.batch_plan_analysis() {
                Some(a) => a,
                None => return Ok(node),
            };

            // Use the underlying optimizer to make decision
            let decision = self.should_materialize(&node, analysis);

            match decision {
                MaterializationDecision::Materialize {
                    reason,
                    reference_count,
                    estimated_rows,
                    materialize_cost,
                    recompute_cost,
                } => {
                    log::debug!(
                        "Materializing CTE: reason={:?}, refs={}, rows={}, cost={:.2} vs {:.2}",
                        reason,
                        reference_count,
                        estimated_rows,
                        materialize_cost,
                        recompute_cost
                    );
                    // Keep the MaterializeNode as-is
                    Ok(node)
                }
                MaterializationDecision::DoNotMaterialize { reason } => {
                    log::debug!("Not materializing CTE: reason={:?}", reason);
                    // Replace MaterializeNode with its input (inline the CTE)
                    Ok(materialize_node.input().clone())
                }
            }
        } else {
            // Pass through non-MaterializeNode
            Ok(node)
        }
    }

    fn name(&self) -> &str {
        "MaterializationOptimizer"
    }

    fn is_enabled(&self) -> bool {
        // Materialization strategy is always enabled
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::StartNode;

    #[test]
    fn test_optimizer_creation() {
        let stats_manager = StatisticsManager::new();
        let optimizer = MaterializationOptimizer::new(&stats_manager);
        assert_eq!(optimizer.min_reference_count, 2);
    }

    #[test]
    fn test_optimizer_with_config() {
        let stats_manager = StatisticsManager::new();
        let optimizer = MaterializationOptimizer::new(&stats_manager)
            .with_min_reference_count(3)
            .with_max_result_rows(5000)
            .with_max_complexity(60);
        assert_eq!(optimizer.min_reference_count, 3);
        assert_eq!(optimizer.max_result_rows, 5000);
        assert_eq!(optimizer.max_complexity, 60);
    }

    #[test]
    fn test_cost_estimation() {
        let stats_manager = StatisticsManager::new();
        let optimizer = MaterializationOptimizer::new(&stats_manager);

        // Test Cost Estimation
        let recompute_cost = optimizer.estimate_recompute_cost(3, 1000);
        let materialize_cost = optimizer.estimate_materialize_cost(1000, 50);

        assert!(recompute_cost > 0.0);
        assert!(materialize_cost > 0.0);
    }

    #[test]
    fn test_memory_cost_configuration() {
        let stats_manager = StatisticsManager::new();
        let optimizer = MaterializationOptimizer::new(&stats_manager)
            .with_memory_cost_factor(0.0002)
            .with_max_memory_cost_ratio(0.3);

        assert_eq!(optimizer.memory_cost_factor, 0.0002);
        assert_eq!(optimizer.max_memory_cost_ratio, 0.3);
    }

    #[test]
    fn test_estimate_memory_cost() {
        let stats_manager = StatisticsManager::new();
        let optimizer = MaterializationOptimizer::new(&stats_manager);

        // Test memory cost estimation
        let memory_cost = optimizer.estimate_memory_cost(1000);
        // 1000 rows * 64 bytes * 0.0001 = 6.4
        assert!(memory_cost > 0.0);
    }

    #[test]
    fn test_should_materialize_single_reference() {
        use crate::query::optimizer::analysis::BatchPlanAnalyzer;

        let stats_manager = StatisticsManager::new();
        let optimizer = MaterializationOptimizer::new(&stats_manager);

        // Create a simple plan
        let root = PlanNodeEnum::Start(StartNode::new());
        let batch_analyzer = BatchPlanAnalyzer::new();
        let analysis = batch_analyzer.analyze(&root);

        // Test with single reference (StartNode is not referenced multiple times)
        let decision = optimizer.should_materialize(&root, &analysis);

        match decision {
            MaterializationDecision::DoNotMaterialize { reason } => {
                assert_eq!(reason, NoMaterializeReason::SingleReference);
            }
            _ => panic!("Expected DoNotMaterialize decision"),
        }
    }
}
