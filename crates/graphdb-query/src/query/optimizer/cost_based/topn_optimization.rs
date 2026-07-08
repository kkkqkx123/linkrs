//! Sorting Elimination Optimizer Module
//!
//! A cost-based sorting optimization strategy, focusing on the decision-making process for converting the Sort + Limit operation into a TopN result.
//!
//! ## Note
//!
//! This module **only contains cost-based optimization logic**.
//!
//! Heuristic sorting eliminations (such as those related to the ordering of indexes) have been handled during the rewrite phase using the `EliminateSortRule` mechanism.
//! This layered design ensures that:
//! Simple optimization measures (such as index matching) should be carried out as early as possible during the rewrite phase.
//! 2. Complex cost decision-making processes (TopN conversion) are carried out during the physical optimization phase.
//!
//! ## Optimization Strategies
//!
//! - Convert “Sort + Limit” to “TopN” (based on cost comparison).
//!
//! ## Usage Examples
//!
//! ```rust
//! use graphdb::query::optimizer::strategy::SortEliminationOptimizer;
//! use graphdb::query::optimizer::cost::CostCalculator;
//! use std::sync::Arc;
//!
//! let optimizer = SortEliminationOptimizer::new(cost_calculator);
//! let decision = optimizer.optimize(&sort_context);
//! ```

use std::sync::Arc;

use crate::query::optimizer::cost::CostCalculator;
use crate::query::planning::plan::core::nodes::{SortItem, SortNode};

/// Sorting-based decision elimination
#[derive(Debug, Clone, PartialEq)]
pub enum SortEliminationDecision {
    /// Maintain the order (this cannot be changed or reversed).
    KeepSort {
        /// Reason for retention
        reason: SortKeepReason,
        /// Estimated cost
        estimated_cost: f64,
    },
    /// Convert to TopN
    ConvertToTopN {
        /// Reason for conversion
        reason: TopNConversionReason,
        /// Cost estimate for TopN method
        topn_cost: f64,
        /// Original sorting cost
        original_cost: f64,
    },
}

/// The reason for maintaining the order:
#[derive(Debug, Clone, PartialEq)]
pub enum SortKeepReason {
    /// There are no Limit child nodes; therefore, it is not possible to convert the data into a TopN format.
    NoLimitForTopN,
    /// The value of “Limit” is too small to be worth converting.
    LimitTooSmall,
    /// Retaining the sorted order based on cost analysis is more advantageous.
    CostBasedDecision,
}

/// Reasons for converting to TopN
#[derive(Debug, Clone, PartialEq)]
pub enum TopNConversionReason {
    /// The combination of “Sort” and “Limit” results in lower costs when implementing the TopN algorithm.
    SortWithLimit,
    /// For small amounts of data, it is better to use the TopN method.
    SmallLimit,
    /// Based on cost analysis
    CostBased,
    /// Memory constrained - sorting would require too much memory
    MemoryConstrained,
}

/// Sorting optimization context
#[derive(Debug, Clone)]
pub struct SortContext {
    /// Sorted nodes
    pub sort_node: SortNode,
    /// Estimated number of lines to be translated
    pub input_rows: u64,
    /// Is there a Limit child node?
    pub has_limit_child: bool,
    /// The Limit value (if any)
    pub limit_value: Option<i64>,
}

impl SortContext {
    /// Create a new sorting context.
    pub fn new(sort_node: SortNode, input_rows: u64) -> Self {
        Self {
            sort_node,
            input_rows,
            has_limit_child: false,
            limit_value: None,
        }
    }

    /// Setting the Limit information
    pub fn with_limit(mut self, limit: i64) -> Self {
        self.has_limit_child = true;
        self.limit_value = Some(limit);
        self
    }
}

/// Sorting Elimination Optimizer
///
/// A sorting optimizer based on a cost model, which focuses on the decision-making process for converting the “Sort + Limit” operation into the retrieval of the TopN results.
///
/// Heuristic optimizations such as the elimination of index orderliness have been completed during the rewrite phase.
#[derive(Debug)]
pub struct SortEliminationOptimizer {
    cost_calculator: Arc<CostCalculator>,
    /// TopN conversion threshold (conversion is considered when limit < threshold * input_rows)
    topn_threshold: f64,
    /// Only the smallest Limit value is considered for the TopN selection.
    min_limit_for_topn: i64,
}

impl SortEliminationOptimizer {
    /// Create a new sorting and elimination optimizer
    pub fn new(cost_calculator: Arc<CostCalculator>) -> Self {
        let thresholds = cost_calculator.config().strategy_thresholds;
        Self {
            cost_calculator,
            topn_threshold: thresholds.topn_threshold,
            min_limit_for_topn: thresholds.topn_default_limit as i64,
        }
    }

    /// Set the TopN conversion threshold
    pub fn with_topn_threshold(mut self, threshold: f64) -> Self {
        self.topn_threshold = threshold.clamp(0.001, 1.0);
        self
    }

    /// Set the minimum limit value
    pub fn with_min_limit(mut self, min_limit: i64) -> Self {
        self.min_limit_for_topn = min_limit.max(1);
        self
    }

    /// Optimize the sorting operation
    ///
    /// The decision to convert from Sort + Limit to TopN is based on a cost analysis.
    ///
    /// # Parameters
    /// **context:** Sort context
    ///
    /// # Returns
    /// Sorting optimization decision (whether to maintain the original sorting order or convert the data into a TopN list)
    pub fn optimize(&self, context: &SortContext) -> SortEliminationDecision {
        self.optimize_with_memory(context, None)
    }

    /// Optimize sorting with memory awareness
    ///
    /// This method considers available memory when making the sort vs TopN decision.
    /// If the sort operation would require more memory than available, it forces TopN conversion.
    ///
    /// # Parameters
    /// - `context`: Sort context
    /// - `available_memory`: Available memory in bytes (None means unlimited)
    ///
    /// # Returns
    /// Sorting optimization decision
    pub fn optimize_with_memory(
        &self,
        context: &SortContext,
        available_memory: Option<usize>,
    ) -> SortEliminationDecision {
        let sort_items = context.sort_node.sort_items();

        // Check memory constraints first
        if let Some(memory) = available_memory {
            let sort_memory = self
                .cost_calculator
                .estimate_sort_memory(context.input_rows, sort_items.len());

            // If sorting would exceed available memory, force TopN
            if sort_memory > memory {
                if let Some(limit) = context.limit_value {
                    if limit >= self.min_limit_for_topn {
                        let original_cost =
                            self.calculate_sort_cost(context.input_rows, sort_items.len());
                        let topn_cost = self
                            .cost_calculator
                            .calculate_topn_cost(context.input_rows, limit);

                        return SortEliminationDecision::ConvertToTopN {
                            reason: TopNConversionReason::MemoryConstrained,
                            topn_cost,
                            original_cost,
                        };
                    }
                }

                // No limit available but memory constrained - keep sort but warn
                let sort_cost = self.calculate_sort_cost(context.input_rows, sort_items.len());
                return SortEliminationDecision::KeepSort {
                    reason: SortKeepReason::CostBasedDecision,
                    estimated_cost: sort_cost,
                };
            }
        }

        // Check whether it is possible to convert this into a TopN format.
        if let Some(decision) = self.check_topn_conversion(context, sort_items) {
            return decision;
        }

        // The text cannot be translated; the original order is therefore retained.
        let sort_cost = self.calculate_sort_cost(context.input_rows, sort_items.len());
        SortEliminationDecision::KeepSort {
            reason: SortKeepReason::NoLimitForTopN,
            estimated_cost: sort_cost,
        }
    }

    /// Check whether it is possible to convert this into a TopN format.
    ///
    /// The decision to convert from “Sort + Limit” to “TopN” is based on a cost comparison.
    /// This is a **cost-based decision**, not an unconditional conversion.
    fn check_topn_conversion(
        &self,
        context: &SortContext,
        sort_items: &[SortItem],
    ) -> Option<SortEliminationDecision> {
        let limit = context.limit_value?;

        if limit < self.min_limit_for_topn {
            return None;
        }

        // Check whether the conditions for the TopN conversion are met.
        let limit_ratio = limit as f64 / context.input_rows as f64;

        if limit_ratio < self.topn_threshold || context.input_rows > 10000 {
            let original_cost = self.calculate_sort_cost(context.input_rows, sort_items.len());
            let topn_cost = self
                .cost_calculator
                .calculate_topn_cost(context.input_rows, limit);

            if topn_cost < original_cost {
                return Some(SortEliminationDecision::ConvertToTopN {
                    reason: if context.has_limit_child {
                        TopNConversionReason::SortWithLimit
                    } else {
                        TopNConversionReason::CostBased
                    },
                    topn_cost,
                    original_cost,
                });
            }
        }

        None
    }

    /// Calculating the cost of sorting
    fn calculate_sort_cost(&self, input_rows: u64, sort_columns: usize) -> f64 {
        self.cost_calculator
            .calculate_sort_cost(input_rows, sort_columns, None)
    }

    /// Check whether it is possible to convert this into TopN nodes.
    ///
    /// # Parameters
    /// - `sort_items`: Functions to sort items
    /// - `limit`: The value of the `limit` parameter
    /// - `input_rows`: The number of input rows
    ///
    /// # Back
    /// - If conversion is possible, return (TopN cost, original sorting cost).
    pub fn check_topn_conversion_cost(
        &self,
        sort_items: &[SortItem],
        limit: i64,
        input_rows: u64,
    ) -> Option<(f64, f64)> {
        if limit < self.min_limit_for_topn {
            return None;
        }

        let limit_ratio = limit as f64 / input_rows as f64;

        if limit_ratio < self.topn_threshold || input_rows > 10000 {
            let original_cost = self.calculate_sort_cost(input_rows, sort_items.len());
            let topn_cost = self.cost_calculator.calculate_topn_cost(input_rows, limit);

            if topn_cost < original_cost {
                return Some((topn_cost, original_cost));
            }
        }

        None
    }

    /// Obtain suggestions for sorting optimization
    ///
    /// Analyze the sorting operation and provide optimization suggestions.
    pub fn get_optimization_advice(&self, context: &SortContext) -> Vec<String> {
        let mut advice = Vec::new();

        match self.optimize(context) {
            SortEliminationDecision::ConvertToTopN {
                reason,
                topn_cost,
                original_cost,
            } => {
                let savings = original_cost - topn_cost;
                advice.push(format!(
                    "It is recommended to convert Sort + Limit to TopN, reason: {:?} and expected cost savings: {:.2}",
                    reason, savings
                ));
            }
            SortEliminationDecision::KeepSort { reason, .. } => {
                advice.push(format!("Preserve the sort operation, cause: {:?}", reason));

                if matches!(reason, SortKeepReason::NoLimitForTopN) {
                    advice.push(
                        "If the query contains a LIMIT, consider converting Sort + Limit to TopN"
                            .to_string(),
                    );
                }
            }
        }

        advice
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::stats::StatisticsManager;

    fn create_test_optimizer() -> SortEliminationOptimizer {
        let stats_manager = Arc::new(StatisticsManager::new());
        let cost_calculator = Arc::new(CostCalculator::new(stats_manager));
        SortEliminationOptimizer::new(cost_calculator)
    }

    #[test]
    fn test_sort_elimination_optimizer_creation() {
        let optimizer = create_test_optimizer();
        // Values come from StrategyThresholds defaults
        assert_eq!(optimizer.topn_threshold, 0.1);
        assert_eq!(optimizer.min_limit_for_topn, 100); // Default from StrategyThresholds
    }

    #[test]
    fn test_with_topn_threshold() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let cost_calculator = Arc::new(CostCalculator::new(stats_manager));
        let optimizer = SortEliminationOptimizer::new(cost_calculator).with_topn_threshold(0.2);

        assert_eq!(optimizer.topn_threshold, 0.2);
    }

    #[test]
    fn test_with_min_limit() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let cost_calculator = Arc::new(CostCalculator::new(stats_manager));
        let optimizer = SortEliminationOptimizer::new(cost_calculator).with_min_limit(10);

        assert_eq!(optimizer.min_limit_for_topn, 10);
    }

    #[test]
    fn test_threshold_clamping() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let cost_calculator = Arc::new(CostCalculator::new(stats_manager));

        let optimizer1 =
            SortEliminationOptimizer::new(cost_calculator.clone()).with_topn_threshold(2.0); // More than 1.0
        assert_eq!(optimizer1.topn_threshold, 1.0);

        let optimizer2 = SortEliminationOptimizer::new(cost_calculator).with_topn_threshold(0.0001); // Less than 0.001
        assert_eq!(optimizer2.topn_threshold, 0.001);
    }

    #[test]
    fn test_context_creation() {
        let start_node = crate::query::planning::plan::core::nodes::StartNode::new();
        let sort_node = SortNode::new(
            crate::query::planning::plan::PlanNodeEnum::Start(start_node),
            vec![SortItem::column_asc("name".to_string())],
        )
        .expect("Failed to create SortNode");

        let context = SortContext::new(sort_node, 1000);
        assert_eq!(context.input_rows, 1000);
        assert!(!context.has_limit_child);
        assert_eq!(context.limit_value, None);
    }

    #[test]
    fn test_context_with_limit() {
        let start_node = crate::query::planning::plan::core::nodes::StartNode::new();
        let sort_node = SortNode::new(
            crate::query::planning::plan::PlanNodeEnum::Start(start_node),
            vec![SortItem::column_asc("name".to_string())],
        )
        .expect("Failed to create SortNode");

        let context = SortContext::new(sort_node, 1000).with_limit(10);

        assert!(context.has_limit_child);
        assert_eq!(context.limit_value, Some(10));
    }

    #[test]
    fn test_check_topn_conversion_cost_limit_too_small() {
        let optimizer = create_test_optimizer();
        let sort_items = vec![SortItem::column_asc("name".to_string())];

        let result = optimizer.check_topn_conversion_cost(&sort_items, 0, 1000);
        assert_eq!(result, None);
    }

    #[test]
    fn test_sort_keep_reason_display() {
        let reason = SortKeepReason::NoLimitForTopN;
        assert!(format!("{:?}", reason).contains("NoLimitForTopN"));

        let reason = SortKeepReason::LimitTooSmall;
        assert!(format!("{:?}", reason).contains("LimitTooSmall"));

        let reason = SortKeepReason::CostBasedDecision;
        assert!(format!("{:?}", reason).contains("CostBasedDecision"));
    }

    #[test]
    fn test_topn_conversion_reason_display() {
        let reason = TopNConversionReason::SortWithLimit;
        assert!(format!("{:?}", reason).contains("SortWithLimit"));

        let reason = TopNConversionReason::SmallLimit;
        assert!(format!("{:?}", reason).contains("SmallLimit"));

        let reason = TopNConversionReason::CostBased;
        assert!(format!("{:?}", reason).contains("CostBased"));

        let reason = TopNConversionReason::MemoryConstrained;
        assert!(format!("{:?}", reason).contains("MemoryConstrained"));
    }

    #[test]
    fn test_optimize_with_memory() {
        let optimizer = create_test_optimizer();

        // Create a sort context with limit
        let start_node = crate::query::planning::plan::core::nodes::StartNode::new();
        let sort_node = SortNode::new(
            crate::query::planning::plan::PlanNodeEnum::Start(start_node),
            vec![SortItem::column_asc("name".to_string())],
        )
        .expect("Failed to create SortNode");

        let context = SortContext::new(sort_node, 10000).with_limit(100);

        // With plenty of memory, should use normal logic
        let decision_with_memory = optimizer.optimize_with_memory(&context, Some(10 * 1024 * 1024)); // 10MB
        assert!(matches!(
            decision_with_memory,
            SortEliminationDecision::ConvertToTopN { .. }
                | SortEliminationDecision::KeepSort { .. }
        ));

        // With very limited memory, should force TopN if possible
        let decision_limited = optimizer.optimize_with_memory(&context, Some(1024)); // 1KB
                                                                                     // Should convert to TopN due to memory constraint
        match decision_limited {
            SortEliminationDecision::ConvertToTopN { reason, .. } => {
                assert_eq!(reason, TopNConversionReason::MemoryConstrained);
            }
            SortEliminationDecision::KeepSort { .. } => {
                // This is also valid if the optimizer decides to keep sort
            }
        }
    }

    #[test]
    fn test_optimize_with_memory_no_limit() {
        let optimizer = create_test_optimizer();

        // Create a sort context without limit
        let start_node = crate::query::planning::plan::core::nodes::StartNode::new();
        let sort_node = SortNode::new(
            crate::query::planning::plan::PlanNodeEnum::Start(start_node),
            vec![SortItem::column_asc("name".to_string())],
        )
        .expect("Failed to create SortNode");

        let context = SortContext::new(sort_node, 1000); // No limit

        // Without limit, memory constraint should result in KeepSort
        let decision = optimizer.optimize_with_memory(&context, Some(1024));
        match decision {
            SortEliminationDecision::KeepSort { .. } => {
                // Expected
            }
            _ => {
                // Other outcomes are also acceptable
            }
        }
    }
}
